//! Hand-written serde types for the Cloud Agents endpoints cox uses, written
//! from https://cursor.com/docs/cloud-agent/api/endpoints (checked
//! 2026-10-03). Never generated from or copied out of Cursor's OpenAPI file,
//! which carries no licence (A123 (5), A40 step 3). The API is a public beta,
//! so every enum keeps an unknown value instead of failing, and fields the
//! docs do not promise are optional: a field Cursor adds or drops must not
//! break a run that is already paid for.
//!
//! Requests carry the prompt, the GitHub URL, the starting ref and the model
//! and nothing that names the user: there is no field for a name, an email or
//! a git identity, so none can be sent by mistake.

// why: the fields and variants mirror the endpoints page linked above one to
// one; restating that page here would only drift from it.
#![allow(missing_docs)]

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// A string-valued enum that keeps a value it does not know as `Other`.
macro_rules! open_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum $name {
            $($variant,)+
            Other(String),
        }

        impl $name {
            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $wire,)+
                    Self::Other(value) => value,
                }
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = String::deserialize(deserializer)?;
                Ok(match value.as_str() {
                    $($wire => Self::$variant,)+
                    _ => Self::Other(value),
                })
            }
        }
    };
}

open_enum! {
    /// A run's lifecycle state.
    RunStatus {
        Creating => "CREATING",
        Running => "RUNNING",
        Finished => "FINISHED",
        Error => "ERROR",
        Cancelled => "CANCELLED",
        Expired => "EXPIRED",
    }
}

impl RunStatus {
    /// Whether the run can no longer change. An unknown status is treated as
    /// live: reading it as terminal could drop a run that is still working.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Finished | Self::Error | Self::Cancelled | Self::Expired
        )
    }
}

open_enum! {
    /// A `tool_call` event's state.
    ToolCallStatus {
        Running => "running",
        Completed => "completed",
    }
}

/// The prompt envelope Cursor expects instead of a bare string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prompt {
    pub text: String,
}

impl Prompt {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

/// The model an agent runs, by Cursor's model id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSelection {
    pub id: String,
}

/// A GitHub repository the agent clones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repo {
    pub url: String,
    #[serde(
        rename = "startingRef",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub starting_ref: Option<String>,
}

/// `POST /v1/agents`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreateAgentRequest {
    pub prompt: Prompt,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelSelection>,
    pub repos: Vec<Repo>,
    /// Always sent, and false unless the caller opts in: an agent that opens
    /// a pull request on its own is an off-machine action the user must ask for.
    #[serde(rename = "autoCreatePR")]
    pub auto_create_pr: bool,
}

impl CreateAgentRequest {
    pub fn new(prompt: impl Into<String>, repo: Repo) -> Self {
        Self {
            prompt: Prompt::new(prompt),
            model: None,
            repos: vec![repo],
            auto_create_pr: false,
        }
    }

    pub fn with_model(mut self, id: impl Into<String>) -> Self {
        self.model = Some(ModelSelection { id: id.into() });
        self
    }
}

/// `POST /v1/agents/{id}/runs`, a follow-up prompt into an existing agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreateRunRequest {
    pub prompt: Prompt,
}

impl CreateRunRequest {
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: Prompt::new(prompt),
        }
    }
}

/// The agent fields cox reads from a create response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Agent {
    pub id: String,
    #[serde(rename = "latestRunId", default)]
    pub latest_run_id: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

/// A branch the run pushed, and its pull request when it opened one.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Branch {
    #[serde(rename = "repoUrl")]
    pub repo_url: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(rename = "prUrl", default)]
    pub pr_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct GitInfo {
    #[serde(default)]
    pub branches: Vec<Branch>,
}

/// A run, as `GET .../runs/{runId}` and the create responses return it.
/// `result` and `duration_ms` appear on terminal runs only.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Run {
    pub id: String,
    #[serde(rename = "agentId", default)]
    pub agent_id: Option<String>,
    pub status: RunStatus,
    #[serde(rename = "durationMs", default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub git: Option<GitInfo>,
}

/// The response of `POST /v1/agents`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CreateAgentResponse {
    pub agent: Agent,
    pub run: Run,
}

/// The response of `POST /v1/agents/{id}/runs`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CreateRunResponse {
    pub run: Run,
}

/// The response of `POST .../cancel`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CancelResponse {
    pub id: String,
}

/// Token counts for one run. Cursor reports no cost and no model, so the
/// ledger row built from this is `billed_externally` (A123 (5)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub struct RunUsage {
    #[serde(rename = "inputTokens", default)]
    pub input_tokens: u64,
    #[serde(rename = "outputTokens", default)]
    pub output_tokens: u64,
    #[serde(rename = "cacheWriteTokens", default)]
    pub cache_write_tokens: u64,
    #[serde(rename = "cacheReadTokens", default)]
    pub cache_read_tokens: u64,
    #[serde(rename = "totalTokens", default)]
    pub total_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RunUsageEntry {
    pub id: String,
    #[serde(rename = "usageUuid", default)]
    pub usage_uuid: Option<String>,
    pub usage: RunUsage,
}

/// The response of `GET /v1/agents/{id}/usage`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct UsageResponse {
    #[serde(rename = "totalUsage", default)]
    pub total_usage: RunUsage,
    #[serde(default)]
    pub runs: Vec<RunUsageEntry>,
}

/// Which parts of a tool call Cursor cut for size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub struct Truncated {
    #[serde(default)]
    pub args: bool,
    #[serde(default)]
    pub result: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ToolCall {
    #[serde(rename = "callId")]
    pub call_id: String,
    pub name: String,
    pub status: ToolCallStatus,
    #[serde(default)]
    pub args: Option<Value>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub truncated: Option<Truncated>,
}

/// The final `result` event.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RunResult {
    #[serde(rename = "runId")]
    pub run_id: String,
    pub status: RunStatus,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(rename = "durationMs", default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub git: Option<GitInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct StatusPayload {
    status: RunStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct TextPayload {
    #[serde(default)]
    text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct ErrorPayload {
    #[serde(default)]
    code: Value,
    #[serde(default)]
    message: String,
}

/// One server-sent event of a run's stream. The event name travels in the SSE
/// `event:` field and the payload in `data:`, so [`StreamEvent::parse`] takes
/// both. Every string inside is untrusted until it passes `cox_sanitize`.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    Status(RunStatus),
    Assistant {
        text: String,
    },
    Thinking {
        text: String,
    },
    ToolCall(ToolCall),
    /// An SDK-shaped object cox does not interpret.
    InteractionUpdate(Value),
    Heartbeat,
    Result(RunResult),
    Error {
        code: String,
        message: String,
    },
    Done,
    /// An event name this version does not know, kept so the driver can skip
    /// it without losing the stream.
    Unknown {
        event: String,
        data: String,
    },
}

impl StreamEvent {
    /// Parses one frame. A documented event whose payload does not fit its
    /// type is an error; an undocumented event name is `Unknown`.
    pub fn parse(event: &str, data: &str) -> Result<Self, serde_json::Error> {
        Ok(match event {
            "status" => Self::Status(serde_json::from_str::<StatusPayload>(data)?.status),
            "assistant" => Self::Assistant {
                text: serde_json::from_str::<TextPayload>(data)?.text,
            },
            "thinking" => Self::Thinking {
                text: serde_json::from_str::<TextPayload>(data)?.text,
            },
            "tool_call" => Self::ToolCall(serde_json::from_str(data)?),
            "interaction_update" => Self::InteractionUpdate(serde_json::from_str(data)?),
            "heartbeat" => Self::Heartbeat,
            "result" => Self::Result(serde_json::from_str(data)?),
            "error" => {
                let payload: ErrorPayload = serde_json::from_str(data)?;
                let code = match payload.code {
                    Value::String(code) => code,
                    Value::Null => String::new(),
                    other => other.to_string(),
                };
                Self::Error {
                    code,
                    message: payload.message,
                }
            }
            "done" => Self::Done,
            other => Self::Unknown {
                event: other.to_owned(),
                data: data.to_owned(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STREAM: &str = include_str!("../tests/fixtures/stream_events.json");

    fn frames() -> Vec<(String, String)> {
        let frames: Vec<Value> = serde_json::from_str(STREAM).expect("fixture is JSON");
        frames
            .iter()
            .map(|frame| {
                (
                    frame["event"].as_str().expect("event name").to_owned(),
                    frame["data"].to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn create_agent_request_has_no_identity_fields() {
        let request = CreateAgentRequest::new(
            "fix the flaky test",
            Repo {
                url: "https://github.com/pyrlyn/cox".to_owned(),
                starting_ref: Some("main".to_owned()),
            },
        )
        .with_model("composer-2");
        let json = serde_json::to_value(&request).expect("serializes");
        assert_eq!(
            json,
            serde_json::json!({
                "prompt": { "text": "fix the flaky test" },
                "model": { "id": "composer-2" },
                "repos": [{ "url": "https://github.com/pyrlyn/cox", "startingRef": "main" }],
                "autoCreatePR": false,
            })
        );
    }

    #[test]
    fn follow_up_run_request_carries_only_the_prompt() {
        let json = serde_json::to_value(CreateRunRequest::new("now add a test")).expect("ok");
        assert_eq!(
            json,
            serde_json::json!({ "prompt": { "text": "now add a test" } })
        );
    }

    #[test]
    fn auto_create_pr_defaults_off() {
        let request = CreateAgentRequest::new(
            "x",
            Repo {
                url: "https://github.com/a/b".to_owned(),
                starting_ref: None,
            },
        );
        assert!(!request.auto_create_pr);
        let json = serde_json::to_value(&request).expect("serializes");
        assert_eq!(json["autoCreatePR"], false);
        assert!(json["repos"][0].get("startingRef").is_none());
    }

    #[test]
    fn create_agent_response_parses_agent_and_run() {
        let body = include_str!("../tests/fixtures/create_agent_response.json");
        let response: CreateAgentResponse = serde_json::from_str(body).expect("parses");
        assert_eq!(response.agent.id, "bc-1");
        assert_eq!(response.agent.latest_run_id.as_deref(), Some("run-1"));
        assert_eq!(response.run.status, RunStatus::Creating);
        assert!(!response.run.status.is_terminal());
    }

    #[test]
    fn terminal_run_parses_result_and_git() {
        let body = include_str!("../tests/fixtures/get_run_finished.json");
        let run: Run = serde_json::from_str(body).expect("parses");
        assert_eq!(run.status, RunStatus::Finished);
        assert!(run.status.is_terminal());
        assert_eq!(run.duration_ms, Some(41_000));
        assert_eq!(run.result.as_deref(), Some("Fixed the flaky test."));
        let branches = run.git.expect("git").branches;
        assert_eq!(branches[0].branch.as_deref(), Some("cursor/fix-flaky"));
        assert_eq!(branches[0].pr_url, None);
    }

    #[test]
    fn unknown_run_status_is_kept_as_other_and_is_not_terminal() {
        let status: RunStatus = serde_json::from_str("\"PAUSED\"").expect("parses");
        assert_eq!(status, RunStatus::Other("PAUSED".to_owned()));
        assert!(!status.is_terminal());
        assert_eq!(serde_json::to_string(&status).expect("ok"), "\"PAUSED\"");
    }

    #[test]
    fn run_usage_parses_the_token_counts() {
        let body = include_str!("../tests/fixtures/usage.json");
        let usage: UsageResponse = serde_json::from_str(body).expect("parses");
        assert_eq!(
            usage.total_usage,
            RunUsage {
                input_tokens: 1200,
                output_tokens: 340,
                cache_write_tokens: 50,
                cache_read_tokens: 800,
                total_tokens: 2390,
            }
        );
        assert_eq!(usage.runs.len(), 1);
        assert_eq!(usage.runs[0].id, "run-1");
        assert_eq!(usage.runs[0].usage_uuid.as_deref(), Some("u-1"));
        assert_eq!(usage.runs[0].usage, usage.total_usage);
    }

    #[test]
    fn cancel_response_parses_the_id() {
        let cancelled: CancelResponse = serde_json::from_str("{\"id\":\"run-1\"}").expect("ok");
        assert_eq!(cancelled.id, "run-1");
    }

    #[test]
    fn stream_event_parses_every_documented_type() {
        let events: Vec<StreamEvent> = frames()
            .iter()
            .filter(|(name, _)| name != "future_event")
            .map(|(name, data)| StreamEvent::parse(name, data).expect(name))
            .collect();
        assert_eq!(events.len(), 9);
        assert_eq!(events[0], StreamEvent::Status(RunStatus::Running));
        assert_eq!(
            events[1],
            StreamEvent::Assistant {
                text: "Looking at the test.".to_owned()
            }
        );
        assert_eq!(
            events[2],
            StreamEvent::Thinking {
                text: "It races.".to_owned()
            }
        );
        let StreamEvent::ToolCall(call) = &events[3] else {
            panic!("expected a tool call, got {:?}", events[3]);
        };
        assert_eq!(call.call_id, "c1");
        assert_eq!(call.status, ToolCallStatus::Completed);
        assert_eq!(
            call.truncated,
            Some(Truncated {
                args: false,
                result: true
            })
        );
        assert!(matches!(events[4], StreamEvent::InteractionUpdate(_)));
        assert_eq!(events[5], StreamEvent::Heartbeat);
        let StreamEvent::Result(result) = &events[6] else {
            panic!("expected a result, got {:?}", events[6]);
        };
        assert_eq!(result.status, RunStatus::Finished);
        assert_eq!(result.text.as_deref(), Some("Fixed it."));
        assert_eq!(
            events[7],
            StreamEvent::Error {
                code: "rate_limited".to_owned(),
                message: "slow down".to_owned()
            }
        );
        assert_eq!(events[8], StreamEvent::Done);
    }

    #[test]
    fn unknown_stream_event_is_kept_as_unknown() {
        let (name, data) = frames()
            .into_iter()
            .find(|(name, _)| name == "future_event")
            .expect("fixture has an undocumented event");
        assert_eq!(
            StreamEvent::parse(&name, &data).expect("never an error"),
            StreamEvent::Unknown { event: name, data }
        );
    }

    #[test]
    fn malformed_documented_event_is_an_error() {
        assert!(StreamEvent::parse("status", "{\"status\":7}").is_err());
        assert!(StreamEvent::parse("result", "{}").is_err());
    }
}
