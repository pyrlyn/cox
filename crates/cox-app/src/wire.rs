// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The app-server wire protocol (DT§4.4, T52.18): the requests a client
//! sends `cox app-server --stdio`, the responses it gets back and the
//! notifications the server pushes, one JSON object per line. Separate from
//! `server` so a client (the remote workspace, T52.20) links the shapes
//! without the serving loop. The payloads are the existing patch, block and
//! workspace types, serialized as the FFI already exports them: there is no
//! second protocol. No message carries a secret; a key never crosses the
//! wire (the remote machine resolves its own).

use std::path::PathBuf;

use cox_protocol::ids::{ArchiveId, SessionId};
use cox_protocol::types::TodoItem;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    Block, Changes, Completion, InboxItem, Intent, Project, SearchHit, SessionEntry, TimelinePatch,
};

/// The protocol version every line carries. A peer refuses a line with
/// another version instead of guessing at its shape.
pub const VERSION: u32 = 1;

/// One line on the wire, either direction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Line {
    Request(Request),
    Response(Response),
    Notification(Notification),
}

/// A client's call; `id` pairs it with its response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Request {
    pub v: u32,
    pub id: u64,
    pub call: Call,
}

/// What `App`, its `Workspace` and a `LiveSession` answer, one variant per
/// method the desktop app calls over the FFI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Call {
    Projects {
        limit: i64,
    },
    Sessions {
        project: PathBuf,
        limit: i64,
    },
    Search {
        query: String,
        limit: i64,
    },
    /// A new session in `cwd`, or `resume`'s; its patches then stream as
    /// `patches` notifications.
    Open {
        cwd: PathBuf,
        resume: Option<SessionId>,
        theme: String,
    },
    Send {
        session: SessionId,
        #[schemars(with = "Value")]
        intent: Intent,
    },
    Snapshot {
        session: SessionId,
    },
    /// A truncated output in full, as `cox expand` prints it.
    Expand {
        session: SessionId,
        archive: ArchiveId,
    },
    Complete {
        session: SessionId,
        token: String,
        limit: u32,
    },
    Changes {
        session: SessionId,
    },
    Plan {
        session: SessionId,
    },
    /// Stops the session's patch stream; the session keeps running.
    Close {
        session: SessionId,
    },
}

/// The answer to request `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Response {
    pub v: u32,
    pub id: u64,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum Outcome {
    Ok(Reply),
    /// The error's text; the client shows it as the FFI's error would be.
    Err(String),
}

/// A call's result, named as the call. The payload types are the FFI's own;
/// the schema leaves their fields to those types (`Value`), so it pins the
/// envelope without a second copy of every block shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Reply {
    Projects(#[schemars(with = "Vec<Value>")] Vec<Project>),
    Sessions(#[schemars(with = "Vec<Value>")] Vec<SessionEntry>),
    Search(#[schemars(with = "Vec<Value>")] Vec<SearchHit>),
    Opened {
        session: SessionId,
        #[schemars(with = "Vec<Value>")]
        blocks: Vec<Block>,
        warnings: Vec<String>,
    },
    /// A fork or handoff answers with its child, already open here.
    Sent {
        child: Option<SessionId>,
    },
    Snapshot(#[schemars(with = "Vec<Value>")] Vec<Block>),
    Expand(String),
    Complete(#[schemars(with = "Vec<Value>")] Vec<Completion>),
    Changes(#[schemars(with = "Value")] Changes),
    Plan(Vec<TodoItem>),
    Closed,
}

/// Pushed by the server, never answered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Notification {
    pub v: u32,
    pub event: ServerEvent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum ServerEvent {
    /// One coalesced batch of a session's timeline, as `next_patches`
    /// returns it.
    Patches {
        session: SessionId,
        #[schemars(with = "Vec<Value>")]
        patches: Vec<TimelinePatch>,
    },
    /// The session's stream ended.
    Ended { session: SessionId },
    /// `Host::notify`: a new inbox item and the badge.
    Inbox {
        #[schemars(with = "Value")]
        item: Box<InboxItem>,
        badge: u32,
    },
    /// `Host::badge`.
    Badge { badge: u32 },
    /// `Host::open_url`: the client opens it only after the user confirms,
    /// and only an `http(s)` URL (T52.19).
    OpenUrl { url: String },
}

/// Why a line could not be read.
#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("not a protocol line: {0}")]
    Json(#[from] serde_json::Error),
    #[error("protocol version {got}, expected {VERSION}")]
    Version { got: u32 },
}

impl Line {
    /// The version the line carries.
    pub fn version(&self) -> u32 {
        match self {
            Self::Request(r) => r.v,
            Self::Response(r) => r.v,
            Self::Notification(n) => n.v,
        }
    }
}

/// `line` as one JSON object and a newline: serde_json escapes every
/// newline inside strings, so a line never splits.
pub fn encode(line: &Line) -> Result<String, WireError> {
    let mut text = serde_json::to_string(line)?;
    text.push('\n');
    Ok(text)
}

/// One received line, refused when its version is not ours.
pub fn decode(text: &str) -> Result<Line, WireError> {
    let line: Line = serde_json::from_str(text.trim_end())?;
    match line.version() {
        VERSION => Ok(line),
        got => Err(WireError::Version { got }),
    }
}

/// The schema committed as `docs/app-server.schema.json`.
pub fn schema() -> schemars::Schema {
    schemars::schema_for!(Line)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use cox_protocol::types::TodoState;

    use super::*;

    fn session() -> SessionId {
        SessionId::new()
    }

    fn request(call: Call) -> Line {
        Line::Request(Request {
            v: VERSION,
            id: 7,
            call,
        })
    }

    fn reply(reply: Reply) -> Line {
        Line::Response(Response {
            v: VERSION,
            id: 7,
            outcome: Outcome::Ok(reply),
        })
    }

    fn event(event: ServerEvent) -> Line {
        Line::Notification(Notification { v: VERSION, event })
    }

    fn every_message() -> Vec<Line> {
        let id = session();
        vec![
            request(Call::Projects { limit: 20 }),
            request(Call::Sessions {
                project: PathBuf::from("/work/cox"),
                limit: 20,
            }),
            request(Call::Search {
                query: "retry".into(),
                limit: 5,
            }),
            request(Call::Open {
                cwd: PathBuf::from("/work/cox"),
                resume: Some(id),
                theme: "base16-ocean.dark".into(),
            }),
            request(Call::Send {
                session: id,
                intent: Intent::Interrupt,
            }),
            request(Call::Snapshot { session: id }),
            request(Call::Expand {
                session: id,
                archive: ArchiveId::new(),
            }),
            request(Call::Complete {
                session: id,
                token: "/co".into(),
                limit: 8,
            }),
            request(Call::Changes { session: id }),
            request(Call::Plan { session: id }),
            request(Call::Close { session: id }),
            reply(Reply::Projects(Vec::new())),
            reply(Reply::Sessions(Vec::new())),
            reply(Reply::Search(Vec::new())),
            reply(Reply::Opened {
                session: id,
                blocks: Vec::new(),
                warnings: vec!["line one\nline two".into()],
            }),
            reply(Reply::Sent { child: None }),
            reply(Reply::Snapshot(Vec::new())),
            reply(Reply::Expand("out\n".into())),
            reply(Reply::Complete(vec![Completion {
                insert: "/compact".into(),
                detail: "Summarize the conversation".into(),
            }])),
            reply(Reply::Plan(vec![TodoItem {
                id: "1".into(),
                text: "wire".into(),
                state: TodoState::Pending,
            }])),
            reply(Reply::Closed),
            Line::Response(Response {
                v: VERSION,
                id: 8,
                outcome: Outcome::Err("session is not open here".into()),
            }),
            event(ServerEvent::Patches {
                session: id,
                patches: vec![TimelinePatch::Reset { blocks: Vec::new() }],
            }),
            event(ServerEvent::Ended { session: id }),
            event(ServerEvent::Badge { badge: 2 }),
            event(ServerEvent::OpenUrl {
                url: "https://example.com/login".into(),
            }),
        ]
    }

    #[test]
    fn wire_round_trips_every_message() {
        for line in every_message() {
            let text = encode(&line).expect("encodes");
            assert_eq!(text.matches('\n').count(), 1, "one line: {text}");
            assert!(text.ends_with('\n'));
            assert_eq!(decode(&text).expect("decodes"), line);
        }
    }

    #[test]
    fn wire_refuses_another_version() {
        let text = encode(&request(Call::Projects { limit: 1 }))
            .expect("encodes")
            .replace("\"v\":1", "\"v\":2");
        assert!(matches!(decode(&text), Err(WireError::Version { got: 2 })));
    }

    /// Every tag a line can carry: `method`, `kind`, `event`, `type`
    /// values, the schema's `const`s and `enum`s.
    fn tags(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                if let Some(Value::String(tag)) = map.get("const") {
                    out.push(tag.clone());
                }
                if let Some(Value::Array(all)) = map.get("enum") {
                    out.extend(all.iter().filter_map(|v| v.as_str().map(str::to_owned)));
                }
                for v in map.values() {
                    tags(v, out);
                }
            }
            Value::Array(items) => items.iter().for_each(|v| tags(v, out)),
            _ => {}
        }
    }

    #[test]
    fn wire_has_no_secret_message() {
        let schema = serde_json::to_value(schema()).expect("schema serializes");
        let mut all = Vec::new();
        tags(&schema, &mut all);
        assert!(all.contains(&"projects".to_owned()), "tags found: {all:?}");
        for tag in &all {
            let tag = tag.to_lowercase();
            for word in [
                "secret",
                "keyring",
                "password",
                "api_key",
                "credential",
                "token",
            ] {
                assert!(
                    !tag.contains(word),
                    "a `{tag}` message would carry a secret"
                );
            }
        }
    }

    /// Pins the wire to `docs/app-server.schema.json`, written on the first
    /// run so `git status` shows it for review, as `cox-plugin-api` does.
    #[test]
    fn app_server_schema_drift() {
        let generated = serde_json::to_string_pretty(&schema()).expect("schema serializes") + "\n";
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/app-server.schema.json");
        match std::fs::read_to_string(&path) {
            Ok(committed) => assert_eq!(
                committed, generated,
                "docs/app-server.schema.json is stale; delete it, rerun this test and commit it"
            ),
            Err(_) => std::fs::write(&path, generated).expect("write the schema file"),
        }
    }
}
