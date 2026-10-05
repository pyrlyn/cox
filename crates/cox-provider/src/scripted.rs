// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The `Scripted` provider: serves a `cox-provider-testkit` scenario one
//! entry per provider call, in order, verbatim, so the loop and every test
//! run with no network and no key (D12). The scenario format
//! ([`TurnSpec`]/[`ToolCallSpec`]/[`parse_scenario`]) and the pure event
//! sequence a turn expands to ([`events_for`]) live in
//! `cox-provider-testkit` (T32.11), re-exported here; this file is the
//! `Provider` glue that estimates usage via `crate::tokens::estimate` and
//! enforces that **the scenario is authoritative** — if the loop asks for
//! more calls than the scenario supplies, `stream` returns
//! `ProviderError::Unsupported`, because the scenario is a test's
//! expectation of exactly how many provider calls its turn should make; a
//! silent fallthrough would be a bug credited to a network round-trip that
//! never showed up in tests.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use cox_protocol::errors::ProviderError;
use cox_protocol::traits::Provider;
use cox_protocol::types::{Caps, Content, Job, ModelId, ProviderEvent, ProviderId, Request, Usage};
pub use cox_provider_testkit::scripted::{ToolCallSpec, TurnSpec, events_for, parse_scenario};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::tokens::estimate;

/// A scripted provider: replays a [`TurnSpec`] per provider call, in order.
/// `turns` is a `Mutex<VecDeque>` because `Provider::stream` takes `&self`
/// and the state machine owns no other interior mutability.
pub struct Scripted {
    turns: Mutex<VecDeque<TurnSpec>>,
    /// The model id reported in `MessageStart`; empty means echo the
    /// request's model, which is what loop tests usually want.
    model: String,
}

impl Scripted {
    /// Builds one from a scenario document.
    pub fn from_toml(toml_text: &str, model: impl Into<String>) -> Result<Self, ProviderError> {
        Ok(Self {
            turns: Mutex::new(parse_scenario(toml_text)?.into()),
            model: model.into(),
        })
    }

    /// Loads a scenario file (`COX_SCENARIO` for `from_env`).
    pub fn from_path(
        path: impl AsRef<Path>,
        model: impl Into<String>,
    ) -> Result<Self, ProviderError> {
        let path = path.as_ref();
        let toml_text = std::fs::read_to_string(path).map_err(|e| ProviderError::BadRequest {
            message: format!("scripted scenario {}: {e}", path.display()),
        })?;
        Self::from_toml(&toml_text, model)
    }

    /// `COX_PROVIDER=scripted` plus `COX_SCENARIO=<path>`.
    pub fn from_env() -> Result<Self, ProviderError> {
        let path = std::env::var("COX_SCENARIO").map_err(|_| ProviderError::BadRequest {
            message: "COX_SCENARIO is required when COX_PROVIDER=scripted".into(),
        })?;
        Self::from_path(path, "")
    }

    /// Estimated usage for the ledger for a scripted call: input = the
    /// request estimate (the same heuristic budget pre-checks use), output
    /// = bytes of the scripted text+inputs over the usual ~4 chars/token.
    /// `estimated` stays `true` — this is not a real bill.
    fn usage_for(turn: &TurnSpec, req: &Request) -> Usage {
        let input = estimate(req).tokens;
        let out_bytes = turn.text.as_deref().unwrap_or_default().len()
            + turn
                .tool_calls
                .iter()
                .map(|c| c.input.to_string().len())
                .sum::<usize>();
        Usage {
            input_tokens: input,
            output_tokens: (out_bytes as f64 / 4.0).ceil() as u32,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            estimated: true,
            cost_usd: 0.0,
            latency_ms: 0,
        }
    }
}

/// T34.9: whether `pat` appears in a plain spoken `Content::Text` block of
/// `req`'s transcript — never inside a tool call's echoed `input` JSON or a
/// tool result's text. Both of those can otherwise leak a marker meant for
/// one particular child into an unrelated session's own request: a
/// background `agent` call's dispatch turn echoes its `task` argument back
/// into the *parent's* own history as a `ToolUse`, and the "background task
/// started" result echoes the label built from it back as a `ToolResult`.
/// Restricting the search to `Text` keeps a scenario's markers scoped to
/// the one subagent whose own task text (or reply) actually carries them.
fn request_says(req: &Request, pat: &str) -> bool {
    req.messages
        .iter()
        .flat_map(|m| &m.content)
        .any(|c| matches!(c, Content::Text { text } if text.contains(pat)))
}

/// Whether `turn` carries a marker at all: a marked turn is served only
/// to a request that `claims` it, never by FIFO order.
fn is_marked(turn: &TurnSpec) -> bool {
    turn.when_contains.is_some() || turn.when_system_contains.is_some()
}

/// Whether `req` meets every marker `turn` sets: `when_contains` in its
/// spoken text (`request_says`), `when_system_contains` in a system block
/// (T7.8).
fn claims(turn: &TurnSpec, req: &Request) -> bool {
    turn.when_contains
        .as_deref()
        .is_none_or(|pat| request_says(req, pat))
        && turn
            .when_system_contains
            .as_deref()
            .is_none_or(|pat| req.system.iter().any(|b| b.text.contains(pat)))
}

#[async_trait]
impl Provider for Scripted {
    fn id(&self) -> ProviderId {
        // A scripted run bills nothing; `Local` keeps the ledger row's
        // provider column honest (never matched to a price table).
        ProviderId::Local
    }

    fn capabilities(&self) -> Caps {
        Caps {
            cache: false,
            thinking: false,
            server_tools: false,
            count_tokens: true, // the byte-cost estimate is free
            max_context: u32::MAX,
        }
    }

    /// Stands in for a vision wire (T40.7), so an attached or tool image
    /// reaches the request a test inspects instead of being held back.
    fn accepts_images(&self, _model: &str) -> bool {
        true
    }

    async fn stream(
        &self,
        req: Request,
        sink: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<Usage, ProviderError> {
        let turn = {
            let mut turns = self.turns.lock().unwrap_or_else(|e| e.into_inner());
            // T34.9: a turn tagged `when_contains` is reserved for whichever
            // request's own transcript names it, found anywhere in the
            // queue; an untagged turn is served only once no tagged turn
            // ahead of it claims this request, so a still-unclaimed tagged
            // turn is never stolen by an unrelated caller's plain FIFO ask
            // (`scripted.rs`'s module doc, `cox-provider-testkit`).
            // A113: `TurnSpec::job` keeps a `title` side request from
            // taking a turn the scenario wrote for the conversation.
            let serves = |t: &TurnSpec| match &t.job {
                Some(job) => *job == req.job,
                None => req.job != Job::Title,
            };
            let picked = turns
                .iter()
                .position(|t| serves(t) && is_marked(t) && claims(t, &req))
                .or_else(|| turns.iter().position(|t| serves(t) && !is_marked(t)));
            let index = picked.ok_or_else(|| ProviderError::Unsupported {
                feature: "scripted scenario ran out".into(),
            })?;
            turns.remove(index).expect("index just found by position")
        };
        let turn = if turn.echo_tools {
            let mut names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
            names.sort_unstable();
            TurnSpec {
                text: Some(names.join("\n")),
                ..turn
            }
        } else {
            turn
        };
        let usage = Self::usage_for(&turn, &req);
        let model = if self.model.is_empty() {
            req.model.clone()
        } else {
            ModelId(self.model.clone())
        };
        for event in events_for(&turn, model, usage) {
            // Same channel discipline as the real providers: honour cancel
            // before each send; bail if the receiving end hung up.
            if cancel.is_cancelled() {
                return Err(ProviderError::Cancelled);
            }
            if sink.send(event).await.is_err() {
                return Err(ProviderError::Cancelled);
            }
        }
        if let Some(message) = turn.error {
            return Err(ProviderError::BadRequest { message });
        }
        Ok(usage)
    }

    async fn count_tokens(&self, req: &Request) -> Result<u32, ProviderError> {
        Ok(estimate(req).tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use cox_protocol::types::{Effort, Job, StopReason, Thinking, Tier};

    fn req() -> Request {
        Request {
            tier: Tier::Code,
            job: Job::Main,
            model: ModelId("req-model".into()),
            system: vec![],
            tools: vec![],
            messages: vec![],
            effort: Effort::High,
            max_tokens: 1024,
            thinking: Thinking::Off,
            cache_breakpoints: vec![],
            stop_sequences: vec![],
        }
    }

    async fn drain(provider: &Scripted, req: Request) -> Result<Vec<ProviderEvent>, ProviderError> {
        let (tx, mut rx) = mpsc::channel(64);
        provider.stream(req, tx, CancellationToken::new()).await?;
        let mut events = Vec::new();
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
        Ok(events)
    }

    #[tokio::test]
    async fn scripted_text_turn_streams_events_in_order() {
        let provider =
            Scripted::from_toml("[[turn]]\ntext = \"hi there\"\n", "").expect("scenario");
        let events = drain(&provider, req()).await.expect("in-scenario");
        let kinds: Vec<&str> = events
            .iter()
            .map(|e| match e {
                ProviderEvent::MessageStart { .. } => "message_start",
                ProviderEvent::TextDelta { .. } => "text_delta",
                ProviderEvent::Stop { .. } => "stop",
                ProviderEvent::Usage { .. } => "usage",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["message_start", "text_delta", "stop", "usage"]);
        assert!(matches!(
            &events[0],
            ProviderEvent::MessageStart { model } if model.0 == "req-model"
        ));
        match &events[3] {
            ProviderEvent::Usage { usage } => assert!(usage.estimated),
            other => panic!("last event must be usage: {other:?}"),
        }
    }

    #[tokio::test]
    async fn scripted_tool_turn_emits_start_delta_end() {
        let toml = "[[turn]]\ntool_calls = [{ name = \"read\", input = { path = \"a.rs\" } }]\n";
        let provider = Scripted::from_toml(toml, "").expect("scenario");
        let events = drain(&provider, req()).await.expect("in-scenario");
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderEvent::ToolUseStart { name, .. } if name == "read"
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderEvent::ToolUseInputDelta { text } if text.contains("a.rs")
        )));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProviderEvent::ToolUseEnd))
        );
        // §1.2: providers report EndTurn even after tool use.
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderEvent::Stop {
                stop: StopReason::EndTurn
            }
        )));
    }

    #[tokio::test]
    async fn scripted_model_override_wins_over_request() {
        let provider =
            Scripted::from_toml("[[turn]]\ntext = \"x\"\n", "script-model").expect("scenario");
        let events = drain(&provider, req()).await.expect("in-scenario");
        assert!(matches!(
            &events[0],
            ProviderEvent::MessageStart { model } if model.0 == "script-model"
        ));
    }

    #[tokio::test]
    async fn scripted_streams_turns_in_order_then_unsupported() {
        let toml = "[[turn]]\ntext = \"one\"\n[[turn]]\ntext = \"two\"\n";
        let provider = Scripted::from_toml(toml, "").expect("scenario");
        let one = drain(&provider, req()).await.expect("turn 1");
        let two = drain(&provider, req()).await.expect("turn 2");
        assert!(one.iter().any(|e| matches!(
            e,
            ProviderEvent::TextDelta { text } if text == "one"
        )));
        assert!(two.iter().any(|e| matches!(
            e,
            ProviderEvent::TextDelta { text } if text == "two"
        )));
        let err = drain(&provider, req())
            .await
            .expect_err("scenario is exhausted");
        assert!(matches!(err, ProviderError::Unsupported { .. }));
    }

    /// A113: a `title` request never takes a conversation turn, and a
    /// turn pinned to `title` never answers the conversation.
    #[tokio::test]
    async fn scripted_title_request_takes_only_a_title_turn() {
        let toml = "[[turn]]\ntext = \"main\"\n[[turn]]\njob = \"title\"\ntext = \"Named\"\n";
        let provider = Scripted::from_toml(toml, "").expect("scenario");
        let title = || Request {
            job: Job::Title,
            ..req()
        };
        let named = drain(&provider, title()).await.expect("title turn");
        assert!(named.iter().any(|e| matches!(
            e,
            ProviderEvent::TextDelta { text } if text == "Named"
        )));
        let err = drain(&provider, title())
            .await
            .expect_err("no title turn left");
        assert!(matches!(err, ProviderError::Unsupported { .. }));
        let main = drain(&provider, req()).await.expect("main turn");
        assert!(main.iter().any(|e| matches!(
            e,
            ProviderEvent::TextDelta { text } if text == "main"
        )));
    }

    /// A request whose transcript carries one plain user text block, the
    /// shape a subagent's own first provider call has (its task text as a
    /// `Content::Text` message, `subagent.rs`'s `run_task`/`drive`).
    fn req_with_text(text: &str) -> Request {
        Request {
            messages: vec![cox_protocol::types::Message {
                role: cox_protocol::types::Role::User,
                content: vec![Content::Text {
                    text: text.to_string(),
                }],
            }],
            ..req()
        }
    }

    /// T34.9: `when_contains` lets a scenario pin a turn to one particular
    /// caller even out of file order — the claim `request_says` restricts
    /// the search to `Content::Text` guards: a "B"-marked turn must not
    /// answer a request that only *mentions* "B" inside a tool call's
    /// echoed JSON input, never in plain text.
    #[tokio::test]
    async fn when_contains_picks_the_marked_turn_out_of_order() {
        let toml = "[[turn]]\ntext = \"for B\"\nwhen_contains = \"TASK-B\"\n\n\
                    [[turn]]\ntext = \"for parent\"\n";
        let provider = Scripted::from_toml(toml, "").expect("scenario");
        // The parent's own request (no "TASK-B" anywhere) must fall through
        // to the untagged turn, never stealing the one reserved for B.
        let parent_events = drain(&provider, req()).await.expect("parent's turn");
        assert!(parent_events.iter().any(|e| matches!(
            e,
            ProviderEvent::TextDelta { text } if text == "for parent"
        )));
        // B's own request, asking second, still finds its marked turn.
        let b_events = drain(&provider, req_with_text("TASK-B: reply"))
            .await
            .expect("b's turn");
        assert!(b_events.iter().any(|e| matches!(
            e,
            ProviderEvent::TextDelta { text } if text == "for B"
        )));
    }

    /// T7.8: a `when_system_contains` turn answers only a request whose
    /// system blocks carry the marker; the same words in the transcript do
    /// not count.
    #[tokio::test]
    async fn when_system_contains_matches_system_blocks_only() {
        let toml = "[[turn]]\ntext = \"ruled\"\nwhen_system_contains = \"RULE-7\"\n";
        let provider = Scripted::from_toml(toml, "").expect("scenario");
        drain(&provider, req_with_text("RULE-7"))
            .await
            .expect_err("a transcript mention is not a system block");
        let ruled = Request {
            system: vec![cox_protocol::types::SystemBlock {
                text: "# Instructions\nRULE-7".into(),
                cache: true,
            }],
            ..req()
        };
        let events = drain(&provider, ruled).await.expect("system turn");
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderEvent::TextDelta { text } if text == "ruled"
        )));
    }

    /// The same guard the module doc calls out: a request that only echoes
    /// the marker inside a tool call's `input` (never plain `Content::Text`)
    /// must not match — otherwise a background `agent` dispatch's own
    /// echoed `task` argument, or its "background task started" result,
    /// would let the parent steal a turn reserved for the child it named.
    #[tokio::test]
    async fn when_contains_ignores_a_marker_inside_tool_use_or_tool_result() {
        let toml = "[[turn]]\ntext = \"for B\"\nwhen_contains = \"TASK-B\"\n\n\
                    [[turn]]\ntext = \"for parent\"\n";
        let provider = Scripted::from_toml(toml, "").expect("scenario");
        let echoing = Request {
            messages: vec![cox_protocol::types::Message {
                role: cox_protocol::types::Role::Assistant,
                content: vec![
                    Content::ToolUse {
                        id: cox_protocol::ids::CallId::new(),
                        name: "agent".into(),
                        input: serde_json::json!({"task": "TASK-B: reply"}),
                    },
                    Content::ToolResult {
                        call_id: cox_protocol::ids::CallId::new(),
                        content: "background task started: TASK-B: reply".into(),
                        is_error: false,
                    },
                ],
            }],
            ..req()
        };
        let events = drain(&provider, echoing).await.expect("parent's turn");
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderEvent::TextDelta { text } if text == "for parent"
        )));
    }

    #[tokio::test]
    async fn scripted_cancel_before_first_send_errs() {
        let provider = Scripted::from_toml("[[turn]]\ntext = \"x\"\n", "").expect("scenario");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = provider
            .stream(req(), mpsc::channel(8).0, cancel)
            .await
            .expect_err("cancelled");
        assert!(matches!(err, ProviderError::Cancelled));
    }

    #[tokio::test]
    async fn scripted_count_tokens_uses_estimate() {
        let provider = Scripted::from_toml("[[turn]]\ntext = \"x\"\n", "").expect("scenario");
        assert!(provider.capabilities().count_tokens);
        assert!(!provider.capabilities().cache);
        let n = provider.count_tokens(&req()).await.expect("estimate");
        assert_eq!(n, crate::tokens::estimate(&req()).tokens);
    }

    #[test]
    fn no_secrets_in_fixtures() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut hits = Vec::new();
        for dir in ["fixtures", "cassettes"] {
            walk_secrets(&root.join(dir), &mut hits);
        }
        assert!(
            hits.is_empty(),
            "secret-shaped bytes in {}",
            hits.iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    fn walk_secrets(dir: &Path, hits: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk_secrets(&path, hits);
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if crate::replay::redact_secrets(&text) != text {
                hits.push(path);
            }
        }
    }
}
