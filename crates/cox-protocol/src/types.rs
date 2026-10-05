// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The wire contract: `Submission` in, `Event` out (plan.md §1.2), plus
//! every type reachable from them and from `Request`/`ToolSpec`. This file
//! has no logic — only shapes and their serde/schemars derives — because
//! `cox-protocol` is the one crate every other crate may depend on, and a
//! behaviourless contract is what keeps that dependency safe to add.
//!
//! Serde convention: struct-shaped enums use `#[serde(tag = "type",
//! rename_all = "snake_case")]` so a rollout line greps as
//! `"type":"tool_call_done"`; small field-less config enums use bare
//! `#[serde(rename_all = "snake_case")]`, serializing as a plain string.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errors::CoreError;
use crate::ids::{ArchiveId, CallId, ItemId, SessionId, TaskId, TurnId};

// ---------------------------------------------------------------------
// Field-less config/tag enums
// ---------------------------------------------------------------------

/// Who decided an approval (`ApprovalDecided::by`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DecidedBy {
    /// The user answered an `ApprovalRequired` prompt.
    User,
    /// An `allow`/`deny`/`ask` rule matched.
    Rule,
    /// An `AllowForSession` grant from earlier in the session matched.
    Session,
    /// The permission mode or approval policy decided without a rule.
    Policy,
    /// A `PreToolUse` hook decided (`Block`/`Modify`).
    Hook,
}

/// How risky a tool call is, independent of what it does (plan.md §1.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Cannot change anything cox does not already show the model.
    ReadOnly,
    /// Writes inside the workspace.
    Write,
    /// Runs a process.
    Exec,
    /// Can destroy data or affect more than the immediate subject (`rm -rf`, `apply_patch` deleting > 5 files).
    Destructive,
}

/// Whether a tool may run alongside other calls in the same batch (plan.md §1.3 step 3.d.iv).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Concurrency {
    /// May run in parallel with other `Parallel` calls, up to `core.parallel_tools`.
    Parallel,
    /// Must run alone; other calls in the batch wait.
    Exclusive,
}

/// A routing tier (plan.md §1.4/D5): a job maps to a tier, a tier maps to a model.
///
/// Ordered cheapest first (declaration order), the one tier ordering every
/// "never up" rule compares with: a plugin's model-call grant clamp
/// (PL§7d) and the `route` decision point (PL§4, T33.20).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Haiku-class or local; mechanical work, never chosen for the main coding turn.
    Cheap,
    /// Sonnet by default, Opus when picked; the main coding turn.
    Code,
    /// Fable 5.1 only, only via `/think`/`--deep`, always confirmed.
    Think,
}

/// What a request is *for* (plan.md §1.4); every job is pinned to one tier in config.
///
/// Every variant but `Plugin` is a bare tag (`"main"`, `"compact"`, …), the
/// convention this file's header describes for field-less enums. `Plugin`
/// breaks that shape — it carries the calling plugin's id — so `Job` gets
/// hand-written `Serialize`/`Deserialize`/`JsonSchema` instead of deriving
/// them: the wire and ledger form is still a single string, `"plugin:<id>"`
/// (PL§7d, T33.15), which `to_tag`/`from_tag` (`cox-store`) and `tag`
/// (`cox`'s `stats.rs`) already assume for every `Job` value. Losing `Copy`
/// (a `String` payload cannot be `Copy`) is why call sites that used to
/// read `self.job`/`row.job` as a value now clone it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    /// The main coding turn.
    Main,
    /// A `/think`/`--deep` plan.
    Plan,
    /// Compaction summary.
    Compact,
    /// Session title generation.
    Title,
    /// Tool-result or transcript summarisation.
    Summarize,
    /// Commit message generation.
    Commit,
    /// Memory extraction.
    Memory,
    /// An `explore` subagent.
    Explore,
    /// A background shell/HTTP subagent.
    Shell,
    /// A custom subagent definition (`.cox/agents`/`.claude/agents`,
    /// T34.1): its own `tier`/`model` decides the tier, not this job.
    Agent,
    /// A hook-driven LLM call.
    Hook,
    /// A plugin's own `cox_model_call` (PL§7d, T33.15): the router runs it
    /// at or below the plugin's granted tier, never `think`, and the
    /// ledger row's job tag is `plugin:<id>`. No `[jobs]` entry names it —
    /// its tier comes from the call itself, already grant-clamped.
    Plugin(String),
}

/// The bare tag every non-`Plugin` variant serializes as — kept in one
/// place so the `Serialize`/`Deserialize`/`JsonSchema` impls below and the
/// schema literals agree.
const JOB_TAGS: [(&str, Job); 11] = {
    // A `const` array can't hold a `String`-carrying variant, so this only
    // ever binds the fieldless ones; `Job::Plugin` is handled separately
    // everywhere this table is used.
    [
        ("main", Job::Main),
        ("plan", Job::Plan),
        ("compact", Job::Compact),
        ("title", Job::Title),
        ("summarize", Job::Summarize),
        ("commit", Job::Commit),
        ("memory", Job::Memory),
        ("explore", Job::Explore),
        ("shell", Job::Shell),
        ("agent", Job::Agent),
        ("hook", Job::Hook),
    ]
};

impl Job {
    /// The bare tag this job serializes as: one of the fixed strings above,
    /// or `plugin:<id>` for `Job::Plugin`.
    fn tag(&self) -> std::borrow::Cow<'static, str> {
        match self {
            Job::Plugin(id) => std::borrow::Cow::Owned(format!("plugin:{id}")),
            other => JOB_TAGS
                .iter()
                .find(|(_, job)| job == other)
                .map(|(tag, _)| std::borrow::Cow::Borrowed(*tag))
                .unwrap_or(std::borrow::Cow::Borrowed("")),
        }
    }
}

impl Serialize for Job {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.tag())
    }
}

impl<'de> Deserialize<'de> for Job {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        if let Some((_, job)) = JOB_TAGS.iter().find(|(tag, _)| *tag == s) {
            return Ok(job.clone());
        }
        if let Some(id) = s.strip_prefix("plugin:")
            && !id.is_empty()
        {
            return Ok(Job::Plugin(id.to_string()));
        }
        Err(serde::de::Error::unknown_variant(
            &s,
            &[
                "main",
                "plan",
                "compact",
                "title",
                "summarize",
                "commit",
                "memory",
                "explore",
                "shell",
                "agent",
                "hook",
                "plugin:<id>",
            ],
        ))
    }
}

impl JsonSchema for Job {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("Job")
    }

    fn json_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "What a request is *for* (plan.md §1.4); every job is pinned to one tier in config.",
            "oneOf": [
                { "description": "The main coding turn.", "type": "string", "const": "main" },
                { "description": "A `/think`/`--deep` plan.", "type": "string", "const": "plan" },
                { "description": "Compaction summary.", "type": "string", "const": "compact" },
                { "description": "Session title generation.", "type": "string", "const": "title" },
                { "description": "Tool-result or transcript summarisation.", "type": "string", "const": "summarize" },
                { "description": "Commit message generation.", "type": "string", "const": "commit" },
                { "description": "Memory extraction.", "type": "string", "const": "memory" },
                { "description": "An `explore` subagent.", "type": "string", "const": "explore" },
                { "description": "A background shell/HTTP subagent.", "type": "string", "const": "shell" },
                {
                    "description": "A custom subagent definition (`.cox/agents`/`.claude/agents`,\nT34.1): its own `tier`/`model` decides the tier, not this job.",
                    "type": "string",
                    "const": "agent"
                },
                { "description": "A hook-driven LLM call.", "type": "string", "const": "hook" },
                {
                    "description": "A plugin's own `cox_model_call` (PL§7d, T33.15): the router runs it\nat or below the plugin's granted tier, never `think`, and the\nledger row's job tag is `plugin:<id>`. No `[jobs]` entry names it —\nits tier comes from the call itself, already grant-clamped.",
                    "type": "string",
                    "pattern": "^plugin:.+$"
                }
            ]
        })
    }
}

/// Reasoning effort passed to the provider.
///
/// Ordered `Low < Medium < High < Xhigh` so the router can clamp a tier's
/// effort to the greatest level a model supports (`docs/design/providers.md`).
/// The four levels are models.dev's own, so a catalog row maps without loss
/// (T30.26).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    /// Cheapest, fastest; used for `cheap`-tier jobs.
    Low,
    /// Between `Low` and `High`: models.dev's and the wires' `medium`.
    Medium,
    /// Default for `code`/`think` tiers.
    High,
    /// User-selected for a flagged large refactor.
    Xhigh,
}

impl Effort {
    /// The lowercase name the config, the wire and `/effort` share.
    pub fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
        }
    }

    /// The inverse of `name`; anything else is `None`.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::Xhigh),
            _ => None,
        }
    }
}

/// Extended/adaptive thinking mode for a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Thinking {
    /// No thinking block requested.
    Off,
    /// Provider decides whether and how much to think.
    Adaptive,
}

/// Why a provider call or a turn stopped.
///
/// Reused for both `ProviderEvent::Stop` (one provider call) and
/// `Event::TurnDone` (the whole turn); a provider only ever emits
/// `EndTurn`/`Refusal`/`Error`, the others are added by `cox-core` once it
/// has aggregated multiple calls in a turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StopReason {
    /// The model finished normally.
    EndTurn,
    /// `core.max_turns` provider calls were used up without finishing.
    MaxTurns,
    /// `Submission::Interrupt` cancelled the turn.
    Interrupted,
    /// A budget cap stopped the turn before another call was made.
    Budget,
    /// The model refused to continue.
    Refusal {
        /// The provider's refusal text, if any.
        detail: String,
    },
    /// The turn ended in an unrecoverable error.
    Error,
}

/// Severity of a `Notice` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Informational; no action needed.
    Info,
    /// Something degraded but the turn continued (e.g. a skipped hook).
    Warn,
    /// A budget threshold was crossed.
    Budget,
    /// A trust-boundary guard fired (sanitized output, sandbox denial).
    Security,
}

/// What a `checkpoints` row holds for one path (T26.1): the bytes a tool
/// call was about to overwrite, a file it created, a file it deleted, or
/// the marker that starts a user turn (`path` empty).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointKind {
    /// The pre-image of a file that was changed; `archive` holds its bytes.
    Pre,
    /// The file did not exist before the call.
    Created,
    /// The file existed before the call and is gone after it; `archive` holds it.
    Deleted,
    /// A user turn started; nothing archived.
    Turn,
}

/// One path in an `Event::Checkpoint`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CheckpointFile {
    /// The confined absolute path.
    pub path: PathBuf,
    /// What was recorded for it.
    pub kind: CheckpointKind,
}

/// A file a code rewind left as it is (`Event::Rewound`), and why (A101).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SkippedFile {
    /// The confined absolute path.
    pub path: PathBuf,
    /// Why it was not restored.
    pub reason: SkipReason,
}

impl<'de> Deserialize<'de> for SkippedFile {
    /// Also reads a bare path: rollouts written before A101 carried no
    /// reason, and an unreadable line in the middle fails the whole resume.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Path(PathBuf),
            File { path: PathBuf, reason: SkipReason },
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Path(path) => Self {
                path,
                reason: SkipReason::Failed {
                    error: "no reason recorded".into(),
                },
            },
            Wire::File { path, reason } => Self { path, reason },
        })
    }
}

/// Why a rewind could not restore a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkipReason {
    /// Its pre-image was over the size cap, so no bytes were kept.
    TooLarge,
    /// The path no longer confines to the workspace roots.
    OutsideRoots,
    /// Reading the pre-image or writing the file failed.
    Failed {
        /// The error, as shown to the user.
        error: String,
    },
}

/// `permissions.mode` (plan.md §1.6/§1.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Rules and risk decide as usual.
    Default,
    /// Only `Risk::ReadOnly` is allowed; everything else denies without asking.
    Plan,
    /// `Write` is allowed automatically; `Exec`/`Destructive` still ask unless a safe-command match.
    Auto,
    /// Everything is allowed; flag-only, banner shown.
    Bypass,
}

/// `core.mode` / `--mode` / `/mode` (P42, A73): a named preset over the
/// permission mode and the main tier only. It never filters tools, so the
/// cache prefix stays byte-stable, and it only narrows `permissions.mode`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// The configured permission mode and main tier, unchanged.
    #[default]
    Editor,
    /// `Plan` (read-only tools only) and the `think` main tier, confirmed.
    Architect,
}

/// `permissions.approval` (plan.md §1.6/§1.8 step 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalPolicy {
    /// Anything not covered by an `allow` rule asks.
    Untrusted,
    /// Default: risk-based asking (step 7).
    OnRequest,
    /// `Exec` runs sandboxed without asking; asks only if the sandbox denies.
    OnFailure,
    /// Any `Ask` becomes `Deny` (headless default).
    Never,
}

/// Why `Event::Compacted` happened (plan.md §1.10, T28.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CompactReason {
    /// Inside a turn, before a provider call whose request would exceed
    /// `compact_at × max_context`.
    PreCall,
    /// At the next turn's start, from the last call's reported usage.
    #[default]
    PostTurn,
    /// `/compact` or `Submission::Compact`.
    Manual,
    /// The provider rejected a request as too long; the call retries once.
    ContextTooLong,
}

/// Why `Event::RepoMapBuilt` happened (P43): the only three moments the
/// repo map in system[2] is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RepoMapReason {
    /// Before the first request of a session.
    SessionStart,
    /// `/repomap refresh` produced different bytes.
    Refresh,
    /// Compaction rebuilt it with the prefix it already restarts.
    Compaction,
}

/// `sandbox.mode` (plan.md §1.6/D7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    /// No writes anywhere.
    ReadOnly,
    /// Writes confined to the workspace roots.
    WorkspaceWrite,
    /// No sandbox at all; requires explicit opt-in.
    DangerFullAccess,
}

/// Which hook trigger point fired (Claude Code hook protocol, D4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    /// Before the user's text is pushed onto history; may block or rewrite it.
    UserPromptSubmit,
    /// Before a tool call runs; may `Block` or `Modify` its input.
    PreToolUse,
    /// After a tool call succeeds.
    PostToolUse,
    /// After a tool call fails.
    PostToolUseFailure,
    /// After a turn finishes normally.
    Stop,
    /// Before compaction runs; may `Block` it.
    PreCompact,
    /// After compaction runs.
    PostCompact,
    /// A session opened.
    SessionStart,
    /// A session closed.
    SessionEnd,
    /// The engine escalated a call to the user.
    PermissionRequest,
    /// A subagent started.
    SubagentStart,
    /// A subagent finished.
    SubagentStop,
    /// A notice was shown.
    Notification,
}

impl HookEvent {
    /// Claude Code's name for the event: the config key and `hook_event_name`.
    pub fn name(self) -> &'static str {
        match self {
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::PostToolUseFailure => "PostToolUseFailure",
            Self::Stop => "Stop",
            Self::PreCompact => "PreCompact",
            Self::PostCompact => "PostCompact",
            Self::SessionStart => "SessionStart",
            Self::SessionEnd => "SessionEnd",
            Self::PermissionRequest => "PermissionRequest",
            Self::SubagentStart => "SubagentStart",
            Self::SubagentStop => "SubagentStop",
            Self::Notification => "Notification",
        }
    }
}

/// `sandbox.linux_backend` (plan.md T4.2): which Linux confinement to use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum LinuxBackend {
    /// `bwrap` when it can create namespaces here, else Landlock, else none.
    #[default]
    Auto,
    /// bubblewrap only.
    Bwrap,
    /// Landlock + seccomp only.
    Landlock,
    /// No confinement on Linux; the surface warns and forces `on-request`.
    None,
}

/// `[sandbox]` config, resolved for one call (plan.md §1.6/D7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxPolicy {
    /// The sandbox mode in effect.
    pub mode: SandboxMode,
    /// Whether network access is allowed.
    pub network: bool,
    /// Extra writable roots beyond the workspace.
    pub writable: Vec<PathBuf>,
    /// Paths inside the workspace that stay read-only even in `workspace-write` (`.git`, `.cox`).
    pub readonly_in_workspace: Vec<PathBuf>,
    /// Which Linux backend confines the command; ignored elsewhere.
    pub linux_backend: LinuxBackend,
}

// ---------------------------------------------------------------------
// Structs and tagged enums reachable from `Event`/`Submission`/`Request`
// ---------------------------------------------------------------------

/// A file, image or other blob attached to a `UserTurn`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Attachment {
    /// Display name (usually the original filename).
    pub name: String,
    /// MIME type, e.g. `"image/png"`.
    pub media_type: String,
    /// Base64-encoded bytes.
    pub data_b64: String,
}

/// A unified diff for one file, produced by `edit`/`apply_patch`/`write`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Diff {
    /// The file the diff applies to.
    pub path: PathBuf,
    /// Unified diff text (`---`/`+++`/`@@` form).
    pub unified: String,
}

/// A pointer to a full tool output stored in the archive (plan.md §1.7/D6a).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ArchiveRef {
    /// The archive row's id (`cox expand <id>`).
    pub id: ArchiveId,
    /// Size of the archived payload, in bytes.
    pub bytes: u64,
}

/// A request from the model to run a tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolCall {
    /// Correlates `ToolCallRequested` through `ToolCallDone`.
    pub id: CallId,
    /// The tool's registered name (or `mcp__<server>__<tool>`).
    pub name: String,
    /// The model-supplied arguments, validated against the tool's `input_schema`.
    pub input: Value,
    /// The call's risk classification, used by the permission engine.
    pub risk: Risk,
    /// What permission rules match on: the confined path, command line, URL, or MCP name.
    pub subject: String,
    /// The simple commands a shell `subject` splits into (T36.1). `None`
    /// means the subject is one unit, as for every tool that is not a shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segments: Option<Segments>,
}

/// A compound command line as the permission engine judges it (T36.1): a
/// deny or ask rule matching any command denies or asks, an allow rule or a
/// session grant must cover every command. Plain strings, so the engine
/// stays pure and the shell tool owns the parser.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Segments {
    /// Every simple command in source order, including those nested in a
    /// subshell, a substitution or a loop body.
    pub commands: Vec<String>,
    /// The split cannot vouch for the whole line — a substitution, `eval`,
    /// `sh -c`, a variable assignment, an output redirect to a path, or a
    /// parse error — so no prefix rule or grant may allow it; deny and ask
    /// rules still match `commands`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub opaque: bool,
}

/// The outcome of a finished tool call, as it appears in history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolResult {
    /// Whether the call succeeded.
    pub ok: bool,
    /// What the model sees: possibly truncated, with a pointer trailer.
    pub visible: String,
    /// Where the untruncated output lives, if it was archived.
    pub archive: Option<ArchiveRef>,
    /// Size of the full (pre-truncation) output, in bytes.
    pub bytes: u64,
    /// Wall-clock time the call took.
    pub duration_ms: u64,
    /// A unified diff, for edit-shaped tools.
    pub diff: Option<Diff>,
    /// The tool's machine-readable payload (`ToolOutput.structured`), so a
    /// surface reads data instead of re-parsing `visible` (DT G3). Absent
    /// from rollouts written before it existed. Boxed: it is rare, and every
    /// `Event::ToolCallDone` would otherwise carry its full width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured: Option<Box<Value>>,
}

impl ToolResult {
    /// The list a `todo` result carries in `structured`, or `None` when the
    /// payload is not one.
    pub fn todo_list(&self) -> Option<Vec<TodoItem>> {
        Vec::<TodoItem>::deserialize(self.structured.as_deref()?).ok()
    }
}

/// One row of the `todo` tool's list, the shape of its `structured` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TodoItem {
    /// Unique within the list.
    pub id: String,
    /// What the step is.
    pub text: String,
    /// Where the step stands.
    pub state: TodoState,
}

/// Where a todo step stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TodoState {
    /// Not started.
    Pending,
    /// Being worked on.
    InProgress,
    /// Finished.
    Done,
}

/// An approval decision, for both `Submission::Approve` and `ApprovalDecided`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Decision {
    /// Run the call once.
    Allow,
    /// Run this call and grant future calls with the same tool + subject prefix, for this session.
    AllowForSession,
    /// Refuse the call.
    Deny {
        /// Shown to the model as the tool result.
        reason: String,
    },
    /// Run the call, but with edited input (e.g. a corrected path).
    Edit {
        /// The replacement input.
        input: Value,
    },
}

/// Why an `ApprovalRequired` was raised (plan.md §1.2/§1.8 step 9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Why {
    /// An `ask` rule matched.
    RuleAsk {
        /// The rule string that matched (e.g. `"Bash(git commit:*)"`).
        rule: String,
    },
    /// No rule matched; the call's risk classification requires asking.
    Risk {
        /// The call's risk.
        risk: Risk,
    },
    /// The sandbox denied the call and `approval == on-failure`.
    SandboxDenied {
        /// The sandbox backend's denial detail.
        detail: String,
    },
    /// The active approval policy forces asking regardless of risk.
    Policy {
        /// The policy in effect.
        policy: ApprovalPolicy,
    },
}

/// What kind of transcript item an `Item` is; each variant carries exactly
/// what that item needs to be replayed into history on resume (plan.md
/// §1.7: "resume rebuilds `history` from `ItemStarted`/`ItemDone` pairs").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ItemKind {
    /// The user's turn text plus any attachments.
    UserMessage {
        /// The submitted text.
        text: String,
        /// Attached files/images, if any.
        attachments: Vec<Attachment>,
    },
    /// The assistant's visible reply text.
    AssistantMessage {
        /// The accumulated text (from `TextDelta`s).
        text: String,
    },
    /// An extended-thinking block.
    Thinking {
        /// The accumulated thinking text.
        text: String,
        /// The provider's signature for replaying the block back, if it requires one.
        signature: Option<String>,
    },
    /// A tool call the model requested.
    ToolCall {
        /// The call itself.
        call: ToolCall,
    },
    /// A finished tool call's result.
    ToolResult {
        /// The call this result belongs to.
        call_id: CallId,
        /// The result.
        result: ToolResult,
    },
    /// A compaction summary.
    Summary {
        /// The summary text.
        text: String,
    },
    /// A cox-generated notice (not part of the model-visible transcript).
    Notice {
        /// The notice's severity.
        level: Level,
        /// The notice text.
        text: String,
    },
}

/// One entry in a session's rebuilt history: an id plus its kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Item {
    /// The item's id, shared by its `ItemStarted`/`ItemDone` pair.
    pub id: ItemId,
    /// The turn this item belongs to, if any (notices between turns have none).
    pub turn: Option<TurnId>,
    /// What the item is and its (possibly still-accumulating) content.
    pub kind: ItemKind,
}

/// Per-request token/cost accounting (plan.md §1.2/§1.9); one row per provider call.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Usage {
    /// Input tokens billed at the full rate.
    pub input_tokens: u32,
    /// Output tokens generated.
    pub output_tokens: u32,
    /// Input tokens served from cache (billed at the cache-read rate).
    pub cache_read_tokens: u32,
    /// Input tokens written to cache (billed at the cache-write rate).
    pub cache_write_tokens: u32,
    /// True when the provider reported no usage and cox estimated it.
    pub estimated: bool,
    /// Computed cost of this call, in USD.
    pub cost_usd: f64,
    /// Wall-clock latency of this call.
    pub latency_ms: u64,
}

impl Usage {
    /// Tokens the model actually saw for this call: input + cache read + cache write
    /// (plan.md §1.9: "context_tokens ... writes ... (input + cache read + cache write)").
    /// Excludes `output_tokens`, which the model produced rather than read.
    pub fn context_tokens(&self) -> u32 {
        self.input_tokens
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_write_tokens)
    }
}

/// Where the context of the request about to be sent goes (A98): the
/// model's window and the request's estimated tokens split into the four
/// parts both surfaces draw. Emitted before the reply, so it cannot ride on
/// `Usage`, which only exists after it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContextBreakdown {
    /// The model's context window in tokens; `None` when neither the model
    /// catalog nor the provider knows it.
    pub window: Option<u32>,
    /// The request's estimated tokens; the four parts sum to it exactly.
    pub total: u32,
    /// The system prompt and the volatile environment block.
    pub system: u32,
    /// The tool schemas.
    pub tools: u32,
    /// Instruction files and the skills index.
    pub instructions: u32,
    /// The conversation: verbatim turns, archive pointers and summaries.
    pub history: u32,
    /// Tokens the last call served from cache, as its usage reported.
    pub cached: u32,
}

/// A parsed `.claude/commands/*.md`-style slash command the surface could
/// not resolve to a built-in `Submission` variant, forwarded to `cox-ext`
/// for execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SlashCommand {
    /// The command name, without the leading `/`.
    pub name: String,
    /// Everything after the command name, already tokenized.
    pub args: Vec<String>,
}

/// A hook runner's verdict for one hook invocation (plan.md §1.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HookOutcome {
    /// Proceed unchanged.
    Continue,
    /// Stop the action the hook gated.
    Block {
        /// Shown to the user/model as the reason.
        reason: String,
    },
    /// Proceed with different input (e.g. a rewritten prompt or tool input).
    Modify {
        /// The replacement input.
        input: Value,
    },
    /// The hook itself failed to run; fail-open per D14/AGENTS.md.
    Failed {
        /// What went wrong (timeout, non-zero exit, bad JSON).
        error: String,
    },
}

/// The session an approval comes from (T27.2). A subagent's is relayed to
/// its parent's surface, so the prompt names the agent that is asking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Source {
    /// The session whose call waits.
    pub session: SessionId,
    /// The subagent's name (`explore-2`); `None` for the session the user
    /// is talking to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The subagent's preset (`explore`, `shell`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
}

/// What one cox process on a workspace is doing right now (plan.md A14).
/// Written by `cox_ext::presence` from the hook seam, read by every other
/// session of the same project and by the TUI's `/agents`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Presence {
    /// The session writing the record.
    pub session: SessionId,
    /// Its process id; informational — the heartbeat decides liveness.
    pub pid: u32,
    /// Where it started.
    pub cwd: PathBuf,
    /// The workspace root it shares with the others (git root, else cwd).
    pub project: PathBuf,
    /// What it is doing.
    pub status: PresenceStatus,
    /// Turns started so far.
    pub turn: u32,
    /// Paths its `edit`/`write` calls changed, newest last.
    pub touched: Vec<String>,
    /// Unix seconds of the last heartbeat.
    pub updated: u64,
    /// The worktree it runs in (`cox --worktree`, T27.3); absent for a
    /// session on the main checkout, and in records older than the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<PathBuf>,
}

/// A session's state as the other sessions see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PresenceStatus {
    /// A turn is running.
    Active,
    /// Parked on an approval the user has not answered.
    Waiting,
    /// Between turns.
    Idle,
    /// No heartbeat for `presence::STALE_SECS`: the process probably died
    /// mid-work, with whatever it was editing left as it was.
    Stopped,
}

impl PresenceStatus {
    /// The lowercase word the model and the status line print.
    pub fn name(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Waiting => "waiting",
            Self::Idle => "idle",
            Self::Stopped => "stopped",
        }
    }
}

/// A submission into the core state machine: everything a surface can ask
/// `cox-core` to do (plan.md §1.2/§1.3).
///
/// # Example
///
/// ```rust
/// use cox_protocol::types::Submission;
///
/// // Pressing Enter in the TUI sends this:
/// let sub = Submission::UserTurn {
///     text: "create hello.txt containing hi".into(),
///     attachments: vec![],
///     confirm_think: false,
/// };
/// // It serialises to the same snake_case JSON the rollout and
/// // `cox run -p --output-format stream-json` use:
/// let json = serde_json::to_value(&sub).unwrap();
/// assert_eq!(json["type"], "user_turn");
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Submission {
    /// Start (or continue) a turn with user text.
    UserTurn {
        /// The submitted text.
        text: String,
        /// Attached files/images.
        attachments: Vec<Attachment>,
        /// `true` runs this one turn on `Tier::Think` (`/think`, the think
        /// toggle, `--deep`), confirming its gate (plan.md invariant #9); the
        /// next turn without it goes back to the session's tier (D5, A103).
        confirm_think: bool,
    },
    /// Answer a pending `ApprovalRequired`.
    Approve {
        /// The call being decided.
        call_id: CallId,
        /// The decision.
        decision: Decision,
    },
    /// The person's reply to a `QuestionAsked` (DT G4).
    Answer {
        /// The `ask_user` call being answered.
        call_id: CallId,
        /// The answer; `None` dismisses the question unanswered.
        text: Option<String>,
    },
    /// Cancel the running turn; tools get the shared cancellation token.
    Interrupt,
    /// Compact the session now, optionally focused on something specific.
    Compact {
        /// What the summary should emphasise, if given.
        focus: Option<String>,
    },
    /// Change the tier's model for the rest of the session.
    SwitchModel {
        /// Which tier to change.
        tier: Tier,
        /// The model to switch to; `None` restores the tier's configured default.
        model: Option<ModelId>,
    },
    /// `/effort`: the effort every main-turn call runs at for the rest of
    /// the session, clamped to what the routed model supports; `None`
    /// restores the tier default (T16.4).
    SetEffort {
        /// The level, or `None` for the tier default.
        effort: Option<Effort>,
    },
    /// Change the active permission mode.
    SetPermissionMode {
        /// The new mode.
        mode: PermissionMode,
    },
    /// Revoke an `AllowForSession` grant (T37.45.3): the next call it
    /// covered goes back through the rules and asks. The core answers with
    /// `GrantRevoked`, which resume replays, so the grant stays gone.
    RevokeGrant {
        /// The grant's tool, as `grants_for` recorded it.
        tool: String,
        /// The grant's subject prefix.
        subject: String,
    },
    /// A slash command the surface parsed but did not resolve itself.
    Command {
        /// The parsed command.
        command: SlashCommand,
    },
    /// A hook runner's result for a hook the core is waiting on.
    HookResult {
        /// The hook invocation's id.
        hook_id: String,
        /// The outcome.
        outcome: HookOutcome,
    },
    /// `/rewind` (T26.2): restore the workspace, the conversation or both
    /// to the start of turn `to_turn`.
    Rewind {
        /// The turn to go back to (its `seq`); that turn and later ones are undone.
        to_turn: u32,
        /// Write every pre-image since `to_turn` back into the workspace.
        code: bool,
        /// Drop the conversation from `to_turn` on (append-only: a marker, not an edit).
        conversation: bool,
    },
    /// `/redo` (T26.4): put back the files the last rewind restored, when
    /// nothing has happened since it. One step; a warning otherwise.
    Redo,
    /// Revert one file (T37.28.3, A101): write back its pre-image from
    /// before `to_turn` first touched it, checkpointing its current bytes
    /// first so the revert can itself be undone. Nothing else changes.
    RevertFile {
        /// Relative to the session's cwd or absolute; confined to the
        /// workspace roots like a tool's path.
        path: String,
        /// The turn to go back before.
        to_turn: u32,
    },
    /// Put one of Review's hunks back (T51.19, A101): hunk `hunk` of the
    /// file's net diff, from its first pre-image since `to_turn` to its
    /// bytes on disk, written back to the pre-image's lines alone. The
    /// current bytes are checkpointed first so the revert can itself be
    /// undone; a file whose [`content_digest`] is no longer `now_digest` is
    /// left as it is, with a Notice.
    RevertHunk {
        /// Relative to the session's cwd or absolute; confined to the
        /// workspace roots like a tool's path.
        path: String,
        /// The turn whose pre-image the diff starts from.
        to_turn: u32,
        /// The hunk's index in Review's diff, from 0.
        hunk: u32,
        /// [`content_digest`] of the bytes Review diffed.
        now_digest: String,
    },
    /// `Ctrl+B` (T27.1): detach a running `bash` or `agent` call into a
    /// background task; the model gets a pointer result and the turn goes on.
    Background {
        /// The running call to detach.
        call_id: CallId,
    },
    /// A composer `!` line (T25.3): `bash` runs under the same hooks,
    /// permission engine and sandbox as a model call. Only `share` (`!!`)
    /// lets the output into history; otherwise the next request is
    /// byte-identical to the one before it.
    UserShell {
        /// The command line, without the `!`/`!!`.
        command: String,
        /// `!!`: append the output to history for the model.
        share: bool,
    },
    /// A composer `@name task` line (T45.5): runs subagent `name` through the
    /// `agent` tool on the model's own path (hooks, permission engine,
    /// budget, agent slots). The line and the answer join history at its
    /// tail, so the next model turn sees them.
    UserAgent {
        /// A preset, agent definition or external agent name.
        name: String,
        /// What the subagent is asked to do.
        task: String,
    },
    /// Wind down the session cleanly.
    Shutdown,
    /// `/rename`, or a rename in the app (A113, T37.22.9): the session's
    /// title, set by the user. The core answers with a `TitleSet` marked
    /// `by_user`, which a generated title never replaces.
    Rename {
        /// The new title; the core keeps its first line, sanitized.
        title: String,
    },
    /// A follow-up for a background task, addressed by `TaskId` (T34.4,
    /// SM§1). Only ever submitted to the **parent** session: `from: None`
    /// is the parent/user, `Some(id)` a sibling relayed through the parent
    /// (SM§3). `hop` is set only by the parent and never trusted from a
    /// tool call (SM§5); T34.5 gives it a router, T34.6 the tool that
    /// produces it.
    TaskMessage {
        /// The task the message is addressed to.
        task: TaskId,
        /// Who it is from; `None` for the parent/user, `Some` for a sibling.
        from: Option<TaskId>,
        /// How many relays this message has been through (SM§5's loop guard).
        hop: u32,
        /// The message text.
        text: String,
    },
}

/// Everything a consumer (TUI, `stream-json`, ACP, the rollout file) can
/// observe from a session (plan.md §1.2/§1.3). Every surface consumes the
/// same stream; nothing is emitted after `TurnDone` for that turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A new session began.
    SessionStarted {
        /// The session's id.
        session: SessionId,
        /// Hash of the effective config, for `resume_builds_identical_request`-style checks.
        config_digest: String,
        /// The working directory the session started in.
        cwd: PathBuf,
    },
    /// A new turn began.
    TurnStarted {
        /// The turn's id.
        turn: TurnId,
        /// The turn's ordinal in this session, 1-based; `/rewind` names
        /// turns by it and the `checkpoints` rows carry it (T26.1).
        #[serde(default)]
        seq: u32,
        /// Which job this turn is (usually `Job::Main`).
        job: Job,
        /// The tier routed to.
        tier: Tier,
        /// The specific model used.
        model: ModelId,
    },
    /// A new transcript item began accumulating.
    ItemStarted {
        /// The item's id.
        item: ItemId,
        /// What kind of item this is (may still be filling in text).
        kind: ItemKind,
    },
    /// Streamed text for an `AssistantMessage` item.
    TextDelta {
        /// The item this delta belongs to.
        item: ItemId,
        /// The next chunk of text.
        text: String,
    },
    /// Streamed text for a `Thinking` item.
    ThinkingDelta {
        /// The item this delta belongs to.
        item: ItemId,
        /// The next chunk of thinking text.
        text: String,
    },
    /// A streamed `Thinking` item stopped growing (A91): how long the model
    /// thought, from its first to its last reasoning delta. Kept in the
    /// rollout, so a replay shows the duration the live run measured.
    ThinkingDone {
        /// The `Thinking` item that ended.
        item: ItemId,
        /// Milliseconds from the first to the last delta.
        duration_ms: u64,
    },
    /// The model requested a tool call.
    ToolCallRequested {
        /// The requested call.
        call: ToolCall,
    },
    /// A tool call needs a decision before it can run.
    ApprovalRequired {
        /// The call awaiting a decision.
        call: ToolCall,
        /// Why it needs one.
        why: Why,
        /// Who is asking (T27.2); `None` only on rollout lines written
        /// before the field existed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<Source>,
    },
    /// `ask_user` waits for the person (DT G4); a `Submission::Answer`
    /// with the same `call_id` resumes it.
    QuestionAsked {
        /// The `ask_user` call asking.
        call_id: CallId,
        /// The question text.
        question: String,
        /// Suggested answers, possibly empty.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        options: Vec<String>,
        /// The subagent asking; `None` for the session the person talks to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<Source>,
    },
    /// A pending approval was decided.
    ApprovalDecided {
        /// The call that was decided.
        call_id: CallId,
        /// The decision.
        decision: Decision,
        /// Who/what decided it.
        by: DecidedBy,
    },
    /// An `AllowForSession` grant was revoked (`Submission::RevokeGrant`).
    GrantRevoked {
        /// The grant's tool.
        tool: String,
        /// The grant's subject prefix.
        subject: String,
    },
    /// Streamed stdout/stderr from a running tool, already sanitised for display.
    ToolCallOutput {
        /// The call producing output.
        call_id: CallId,
        /// The next chunk of output.
        delta: String,
    },
    /// A tool call finished.
    ToolCallDone {
        /// The call that finished.
        call_id: CallId,
        /// Its result.
        result: ToolResult,
    },
    /// A transcript item finished accumulating.
    ItemDone {
        /// The item that finished.
        item: ItemId,
    },
    /// A provider call's usage/cost was recorded.
    Usage {
        /// The turn this usage belongs to.
        turn: TurnId,
        /// The recorded usage.
        usage: Usage,
    },
    /// A request was assembled and is about to be sent (A98): the window
    /// and how the request fills it. Output only; the request is unchanged.
    ContextBreakdown {
        /// The turn the request belongs to.
        turn: TurnId,
        /// The window and the split.
        breakdown: ContextBreakdown,
    },
    /// Compaction ran and replaced older items with a summary.
    Compacted {
        /// The new summary item's id.
        summary: ItemId,
        /// Items now skipped when building requests (rollout keeps them).
        dropped: Vec<ItemId>,
        /// Context tokens before compaction.
        before_tokens: u32,
        /// Context tokens after compaction.
        after_tokens: u32,
        /// What triggered it (T28.3); rollouts written before the field
        /// existed read as `post-turn`.
        #[serde(default)]
        reason: CompactReason,
    },
    /// The repo map now in system[2] (P43). Its text is archived before
    /// this is emitted; resume reads the last one back instead of
    /// rebuilding, so the replayed request carries the same bytes.
    RepoMapBuilt {
        /// The archived map text.
        archive: ArchiveId,
        /// Its size in bytes.
        bytes: u64,
        /// What built it.
        reason: RepoMapReason,
    },
    /// Pre-images of the files a tool call changed are archived and
    /// retrievable (T26.1). Emitted after the `checkpoints` rows exist, so a
    /// surface that sees it can already `/rewind`; never emitted for a call
    /// that changed nothing.
    Checkpoint {
        /// The turn the call ran in.
        turn: TurnId,
        /// The call, or `None` for a rewind's own writes.
        call: Option<CallId>,
        /// Every path recorded, in path order.
        files: Vec<CheckpointFile>,
    },
    /// `/rewind` finished (T26.2). With `conversation`, resume and the
    /// context builder stop reading history at `to_turn`; the rollout keeps
    /// every earlier line.
    Rewound {
        /// The turn the session went back to.
        to_turn: u32,
        /// Files were restored.
        code: bool,
        /// The conversation was cut.
        conversation: bool,
        /// Paths written back or removed.
        restored: Vec<PathBuf>,
        /// Paths left as they are, each with why.
        skipped: Vec<SkippedFile>,
    },
    /// A background task (subagent or detached `bash`) was created.
    TaskCreated {
        /// The task's id.
        task: TaskId,
        /// A short human-readable label.
        label: String,
        /// The tier it runs on.
        tier: Tier,
    },
    /// A background task (subagent or detached `bash`) finished.
    TaskCompleted {
        /// The task that finished.
        task: TaskId,
        /// The item holding its result.
        result_item: ItemId,
        /// What it cost, in USD.
        cost_usd: f64,
        /// A shell task's exit code (T27.1); absent for subagents.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        /// Where a shell task's full output was archived (T27.1).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        archive: Option<ArchiveId>,
    },
    /// A tier's model changed mid-session.
    ModelSwitched {
        /// Which tier changed.
        tier: Tier,
        /// The previous model.
        from: ModelId,
        /// The new model.
        to: ModelId,
    },
    /// The session's permission mode or effort override changed
    /// (`SetPermissionMode`, `SetEffort`); carries both, as they now stand,
    /// so a surface sets its state from the event instead of echoing its
    /// own request (DT G5). Also recorded when a top-level session opens
    /// (T50.4), so resume and a woken subagent rebuild the mode the session
    /// last ran under instead of falling back to `Default` (T50.2).
    StateChanged {
        /// The permission mode now in force.
        mode: PermissionMode,
        /// The effort override; `None` means each tier's default.
        effort: Option<Effort>,
    },
    /// The session's mode changed (`/mode`, P42), or a top-level session
    /// opened under a non-default one; carries the permission mode the
    /// preset left in force so a surface needs no second event.
    ModeChanged {
        /// The mode now in force.
        mode: Mode,
        /// The permission mode now in force, after the preset narrowed it.
        permission_mode: PermissionMode,
    },
    /// The session got a title (DT G6), for the sessions list and a window
    /// or tab title.
    TitleSet {
        /// The title, one line.
        title: String,
        /// Set by `Submission::Rename`: the store keeps it over any later
        /// generated title (A113). Absent in rollouts before T37.22.9.
        #[serde(default)]
        by_user: bool,
    },
    /// A decision plugin answered a decision point (PL§4, T33.20). Recorded
    /// for every answer, used or not, so replay and `cox stats` see which
    /// advice changed the core's pick; it never feeds model history.
    Advised {
        /// Which point asked.
        point: crate::plugin::DecidePoint,
        /// The plugin `[plugins.decide]` names for the point.
        plugin: String,
        /// The plugin's answer as given.
        advice: crate::plugin::Advice,
        /// Whether the core followed it (its choice may equal the static
        /// pick); `false` means it was ignored and the static pick stood.
        applied: bool,
    },
    /// An informational or warning message, not part of the model-visible transcript.
    Notice {
        /// Severity.
        level: Level,
        /// The message.
        text: String,
    },
    /// A turn finished.
    TurnDone {
        /// The turn that finished.
        turn: TurnId,
        /// Why it stopped.
        stop: StopReason,
    },
    /// An error occurred.
    Error {
        /// What went wrong.
        error: CoreError,
        /// Whether the whole session must end (`StoreError::Corrupt`, `Config`) or just the turn.
        fatal: bool,
    },
    /// A follow-up for a background task was delivered or relayed (T34.4,
    /// SM§1); same fields as `Submission::TaskMessage`. Renders as one
    /// transcript line labelled like a relayed approval (T34.7); ACP gives
    /// it its own arm instead of falling into `Ok(_) => {}` (T34.8).
    TaskMessage {
        /// The task the message is addressed to.
        task: TaskId,
        /// Who it is from; `None` for the parent/user, `Some` for a sibling.
        from: Option<TaskId>,
        /// How many relays this message has been through (SM§5's loop guard).
        hop: u32,
        /// The message text.
        text: String,
    },
}

// ---------------------------------------------------------------------
// Provider-neutral request/response shapes (plan.md §1.2)
// ---------------------------------------------------------------------

/// A newtype around a provider's model identifier (e.g. `"claude-sonnet-5"`).
/// Deliberately a bare string, not an enum: models are configured, not
/// compiled in (`config/default.toml` [tiers.*].model).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct ModelId(pub String);

impl std::fmt::Display for ModelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which provider backend is in play; matches the `[providers.*]` config sections (D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    /// Anthropic Messages API.
    Anthropic,
    /// OpenAI Responses or Chat Completions API.
    OpenAi,
    /// A local OpenAI-compatible server (Ollama, vLLM, LM Studio, …).
    Local,
    /// TypeSafe System One API (Jev decision model, T21.1).
    Jev,
    /// An external CLI agent from a plugin (EA§6, T35.5): billed on the
    /// user's own plan, so its ledger rows are `$0` and never priced here.
    External,
}

/// One block of the system prompt, with its own cache eligibility (plan.md §1.9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SystemBlock {
    /// The block's text.
    pub text: String,
    /// Whether this block may sit before a cache breakpoint.
    pub cache: bool,
}

/// Who sent a `Message` in a `Request`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The end user (also carries tool results, by provider convention).
    User,
    /// The model.
    Assistant,
}

/// One piece of a `Message`'s content (plan.md §1.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Content {
    /// Plain text.
    Text {
        /// The text.
        text: String,
    },
    /// An extended-thinking block, replayed back to providers that require it verbatim.
    Thinking {
        /// The thinking text.
        text: String,
        /// The provider's signature for this block, if required.
        signature: Option<String>,
    },
    /// The model's request to use a tool.
    ToolUse {
        /// The call's id.
        id: CallId,
        /// The tool name.
        name: String,
        /// The (possibly still-accumulating) input.
        input: Value,
    },
    /// A tool's result, sent back to the model.
    ToolResult {
        /// Which call this answers.
        call_id: CallId,
        /// The result content (already truncated/sanitised for the model).
        content: String,
        /// Whether the tool call failed.
        is_error: bool,
    },
    /// An inline image.
    Image {
        /// MIME type.
        media_type: String,
        /// Base64-encoded bytes.
        data_b64: String,
    },
    /// A reference to archived content instead of the content itself (microcompaction).
    Pointer {
        /// Where the full content lives.
        archive: ArchiveRef,
        /// A short description shown in its place.
        summary: String,
    },
}

/// One message in a `Request`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Message {
    /// Who sent it.
    pub role: Role,
    /// Its content blocks.
    pub content: Vec<Content>,
}

/// A provider-neutral request; providers translate this to their own wire
/// format, and nothing above `cox-provider` knows what that format is
/// (plan.md §1.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Request {
    /// The routing tier this request was assembled for.
    pub tier: Tier,
    /// The job this request serves.
    pub job: Job,
    /// The specific model to call.
    pub model: ModelId,
    /// System prompt blocks, in cache-stable order (plan.md §1.9).
    pub system: Vec<SystemBlock>,
    /// Available tool specs, already filtered (deferred tools absent unless discovered).
    pub tools: Vec<ToolSpec>,
    /// The conversation so far.
    pub messages: Vec<Message>,
    /// Requested reasoning effort.
    pub effort: Effort,
    /// Max output tokens.
    pub max_tokens: u32,
    /// Extended-thinking mode.
    pub thinking: Thinking,
    /// Indices into `system` + `messages` (in that concatenated order) marking cache breakpoints; at most 3.
    pub cache_breakpoints: Vec<usize>,
    /// Sequences that stop generation.
    pub stop_sequences: Vec<String>,
}

/// One event from a provider's streamed response (plan.md §1.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderEvent {
    /// The stream started; names the model that answered (may differ from the requested alias).
    MessageStart {
        /// The model that is responding.
        model: ModelId,
    },
    /// The next chunk of assistant text.
    TextDelta {
        /// The text chunk.
        text: String,
    },
    /// The next chunk of thinking text.
    ThinkingDelta {
        /// The text chunk.
        text: String,
    },
    /// A tool-use block started.
    ToolUseStart {
        /// The call's id.
        id: CallId,
        /// The tool name.
        name: String,
    },
    /// The current tool-use block's thought signature (Gemini over the Chat
    /// wire). Opaque: cox never reads it, and it is replayed only to the wire
    /// that produced it. Follows its call's `ToolUseStart`, before `ToolUseEnd`.
    ToolUseSignature {
        /// The signature, byte-for-byte as received.
        signature: String,
    },
    /// The next chunk of a tool-use block's JSON input.
    ToolUseInputDelta {
        /// The raw JSON chunk (accumulate and parse once `ToolUseEnd` arrives).
        text: String,
    },
    /// The current tool-use block finished.
    ToolUseEnd,
    /// The stream stopped.
    Stop {
        /// Why it stopped.
        stop: StopReason,
    },
    /// Final usage for this call.
    Usage {
        /// The recorded usage.
        usage: Usage,
    },
    /// The provider is retrying after a transient failure.
    Retrying {
        /// Which retry attempt this is (1-based).
        attempt: u32,
        /// How long cox waited before this attempt.
        after_ms: u64,
    },
    /// The call failed.
    Error {
        /// The failure.
        error: crate::errors::ProviderError,
    },
}

/// What a provider implementation can do; used to skip unsupported request shapes
/// instead of sending them and getting `ProviderError::Unsupported`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Caps {
    /// Supports prompt caching (`cache_control`/automatic prefix caching).
    pub cache: bool,
    /// Supports extended/adaptive thinking.
    pub thinking: bool,
    /// Supports first-party server tools (web search/fetch passthrough).
    pub server_tools: bool,
    /// Supports a dedicated token-counting endpoint.
    pub count_tokens: bool,
    /// The model's max context window, in tokens.
    pub max_context: u32,
}

/// A tool's advertised shape (plan.md §1.2/§1.11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolSpec {
    /// The tool's registered name.
    pub name: String,
    /// Shown to the model.
    pub description: String,
    /// JSON Schema for the tool's input.
    pub input_schema: Value,
    /// True for tools found only through `tool_search`, absent from `system[0]` until discovered.
    pub deferred: bool,
    /// Default risk classification for calls to this tool.
    pub risk: Risk,
    /// Whether calls to this tool may run in parallel with others.
    pub concurrency: Concurrency,
}

/// A tool's raw result, before the core archives and truncates it (plan.md §1.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolOutput {
    /// The full, untruncated output.
    pub text: String,
    /// Whether the call failed.
    pub is_error: bool,
    /// A unified diff, for edit-shaped tools.
    pub diff: Option<Diff>,
    /// Machine-readable payload alongside `text`, for surfaces that want structure.
    pub structured: Option<Value>,
}

/// The digest a hunk revert carries (T51.19): Review takes it of the bytes
/// it diffed and the core of the file on disk, so a hunk is never put back
/// into bytes the person did not see. Both sides run in one binary, so the
/// standard hasher, fixed within a build though unspecified across Rust
/// releases, is enough; 16 hex digits.
pub fn content_digest(bytes: &[u8]) -> String {
    use std::hash::{DefaultHasher, Hash as _, Hasher as _};
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    fn sample_usage() -> Usage {
        Usage {
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 30,
            cache_write_tokens: 5,
            estimated: false,
            cost_usd: 0.01,
            latency_ms: 250,
        }
    }

    #[test]
    fn effort_medium_sits_between_low_and_high_and_round_trips_by_name() {
        let all = [Effort::Low, Effort::Medium, Effort::High, Effort::Xhigh];
        assert!(all.windows(2).all(|w| w[0] < w[1]));
        for e in all {
            assert_eq!(Effort::parse(e.name()), Some(e));
            assert_eq!(serde_json::to_value(e).ok(), Some(e.name().into()));
        }
        assert_eq!(Effort::Medium.name(), "medium");
    }

    #[test]
    fn usage_sums_cache_fields() {
        let usage = sample_usage();
        assert_eq!(usage.context_tokens(), 100 + 30 + 5);
    }

    /// Every fieldless `Job` still serializes as its bare tag (unchanged by
    /// the hand-written impl), and `Plugin` serializes/round-trips as
    /// `plugin:<id>` — the literal ledger tag `to_tag`/`from_tag`
    /// (`cox-store`) and `cox`'s `stats.rs` `tag()` both rely on (T33.15).
    #[test]
    fn job_tags_are_plain_strings_and_plugin_round_trips_by_id() {
        assert_eq!(serde_json::to_value(Job::Main).unwrap(), "main");
        assert_eq!(serde_json::to_value(Job::Hook).unwrap(), "hook");
        assert_eq!(
            serde_json::to_value(Job::Plugin("git-glance".into())).unwrap(),
            "plugin:git-glance"
        );
        let round: Job = serde_json::from_value(serde_json::json!("plugin:git-glance")).unwrap();
        assert_eq!(round, Job::Plugin("git-glance".into()));
        let round: Job = serde_json::from_value(serde_json::json!("main")).unwrap();
        assert_eq!(round, Job::Main);
        assert!(serde_json::from_value::<Job>(serde_json::json!("plugin:")).is_err());
        assert!(serde_json::from_value::<Job>(serde_json::json!("bogus")).is_err());
    }

    #[rstest]
    #[case::session_started(Event::SessionStarted { session: SessionId::new(), config_digest: "deadbeef".into(), cwd: PathBuf::from("/tmp") })]
    #[case::turn_started(Event::TurnStarted { turn: TurnId::new(), seq: 1, job: Job::Main, tier: Tier::Code, model: ModelId("claude-sonnet-5".into()) })]
    #[case::item_started(Event::ItemStarted { item: ItemId::new(), kind: ItemKind::UserMessage { text: "hi".into(), attachments: vec![] } })]
    #[case::text_delta(Event::TextDelta { item: ItemId::new(), text: "chunk".into() })]
    #[case::thinking_delta(Event::ThinkingDelta { item: ItemId::new(), text: "chunk".into() })]
    #[case::thinking_done(Event::ThinkingDone { item: ItemId::new(), duration_ms: 12_000 })]
    #[case::tool_call_requested(Event::ToolCallRequested { call: ToolCall { id: CallId::new(), name: "read".into(), input: serde_json::json!({"path": "a.rs"}), risk: Risk::ReadOnly, subject: "a.rs".into(), segments: None } })]
    #[case::approval_required(Event::ApprovalRequired { call: ToolCall { id: CallId::new(), name: "bash".into(), input: Value::Null, risk: Risk::Exec, subject: "ls".into(), segments: None }, why: Why::Risk { risk: Risk::Exec }, source: Some(Source { session: SessionId::new(), agent: Some("explore-2".into()), preset: Some("explore".into()) }) })]
    #[case::approval_decided(Event::ApprovalDecided { call_id: CallId::new(), decision: Decision::Allow, by: DecidedBy::User })]
    #[case::grant_revoked(Event::GrantRevoked { tool: "bash".into(), subject: "git push".into() })]
    #[case::tool_call_output(Event::ToolCallOutput { call_id: CallId::new(), delta: "stdout line".into() })]
    #[case::tool_call_done(Event::ToolCallDone { call_id: CallId::new(), result: ToolResult { ok: true, visible: "done".into(), archive: None, bytes: 4, duration_ms: 10, diff: None, structured: None } })]
    #[case::item_done(Event::ItemDone { item: ItemId::new() })]
    #[case::usage(Event::Usage { turn: TurnId::new(), usage: sample_usage() })]
    #[case::context_breakdown(Event::ContextBreakdown { turn: TurnId::new(), breakdown: ContextBreakdown { window: Some(200_000), total: 900, system: 100, tools: 500, instructions: 200, history: 100, cached: 0 } })]
    #[case::compacted(Event::Compacted { summary: ItemId::new(), dropped: vec![ItemId::new()], before_tokens: 1000, after_tokens: 200, reason: CompactReason::PreCall })]
    #[case::checkpoint(Event::Checkpoint { turn: TurnId::new(), call: Some(CallId::new()), files: vec![CheckpointFile { path: PathBuf::from("/w/a.rs"), kind: CheckpointKind::Pre }] })]
    #[case::task_created(Event::TaskCreated { task: TaskId::new(), label: "explore".into(), tier: Tier::Cheap })]
    #[case::task_completed(Event::TaskCompleted { task: TaskId::new(), result_item: ItemId::new(), cost_usd: 0.002, exit_code: Some(0), archive: Some(ArchiveId::new()) })]
    #[case::model_switched(Event::ModelSwitched { tier: Tier::Code, from: ModelId("claude-sonnet-5".into()), to: ModelId("claude-opus-5".into()) })]
    #[case::state_changed(Event::StateChanged { mode: PermissionMode::Plan, effort: Some(Effort::Low) })]
    #[case::mode_changed(Event::ModeChanged { mode: Mode::Architect, permission_mode: PermissionMode::Plan })]
    #[case::question_asked(Event::QuestionAsked { call_id: CallId::new(), question: "which?".into(), options: vec!["a".into()], source: None })]
    #[case::title_set(Event::TitleSet { title: "Fix the ledger".into(), by_user: false })]
    #[case::title_set_by_user(Event::TitleSet { title: "Mine".into(), by_user: true })]
    #[case::advised(Event::Advised { point: crate::plugin::DecidePoint::Route, plugin: "jev".into(), advice: crate::plugin::Advice { answer: crate::plugin::Answer::Choice { order: vec![0] }, confidence: Some(0.9), note: None }, applied: true })]
    #[case::notice(Event::Notice { level: Level::Warn, text: "hook skipped".into() })]
    #[case::turn_done(Event::TurnDone { turn: TurnId::new(), stop: StopReason::EndTurn })]
    #[case::error(Event::Error { error: CoreError::Interrupted, fatal: false })]
    #[case::task_message(Event::TaskMessage { task: TaskId::new(), from: Some(TaskId::new()), hop: 1, text: "ping".into() })]
    fn event_json_roundtrip(#[case] event: Event) {
        let json = serde_json::to_string(&event).expect("serialize");
        let back: Event = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, back);
    }

    #[rstest]
    #[case::tool_use_start(ProviderEvent::ToolUseStart { id: CallId::new(), name: "read".into() })]
    #[case::tool_use_signature(ProviderEvent::ToolUseSignature { signature: "sig-opaque".into() })]
    #[case::tool_use_end(ProviderEvent::ToolUseEnd)]
    fn provider_event_json_roundtrip(#[case] event: ProviderEvent) {
        let json = serde_json::to_string(&event).expect("serialize");
        let back: ProviderEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, back);
    }

    #[test]
    fn turn_started_without_seq_defaults_to_zero() {
        let turn = TurnId::new();
        let json = serde_json::json!({
            "type": "turn_started",
            "turn": turn,
            "job": "main",
            "tier": "code",
            "model": "m"
        });
        let event: Event = serde_json::from_value(json).expect("old rollout line");
        assert!(matches!(event, Event::TurnStarted { seq: 0, .. }));
    }

    #[test]
    fn a_skipped_path_without_a_reason_still_loads() {
        let json = serde_json::json!({
            "type": "rewound", "to_turn": 1, "code": true, "conversation": false,
            "restored": [], "skipped": ["big.bin"]
        });
        let event: Event = serde_json::from_value(json).expect("old rollout line");
        let Event::Rewound { skipped, .. } = event else {
            panic!("not a rewind: {event:?}");
        };
        assert_eq!(skipped[0].path, PathBuf::from("big.bin"));
        assert!(matches!(skipped[0].reason, SkipReason::Failed { .. }));
        let now = SkippedFile {
            path: "a.rs".into(),
            reason: SkipReason::OutsideRoots,
        };
        let back: SkippedFile =
            serde_json::from_value(serde_json::to_value(&now).expect("ser")).expect("de");
        assert_eq!(back, now);
    }

    #[rstest]
    #[case::user_turn(Submission::UserTurn { text: "fix the bug".into(), attachments: vec![], confirm_think: false })]
    #[case::approve(Submission::Approve { call_id: CallId::new(), decision: Decision::Deny { reason: "no".into() } })]
    #[case::answer(Submission::Answer { call_id: CallId::new(), text: Some("yes".into()) })]
    #[case::answer_dismissed(Submission::Answer { call_id: CallId::new(), text: None })]
    #[case::interrupt(Submission::Interrupt)]
    #[case::compact(Submission::Compact { focus: Some("auth flow".into()) })]
    #[case::switch_model(Submission::SwitchModel { tier: Tier::Code, model: Some(ModelId("claude-opus-5".into())) })]
    #[case::set_effort(Submission::SetEffort { effort: Some(Effort::Xhigh) })]
    #[case::set_permission_mode(Submission::SetPermissionMode { mode: PermissionMode::Plan })]
    #[case::revoke_grant(Submission::RevokeGrant { tool: "bash".into(), subject: "git push".into() })]
    #[case::command(Submission::Command { command: SlashCommand { name: "compact".into(), args: vec![] } })]
    #[case::hook_result(Submission::HookResult { hook_id: "pre-tool-use".into(), outcome: HookOutcome::Continue })]
    #[case::revert_file(Submission::RevertFile { path: "src/a.rs".into(), to_turn: 2 })]
    #[case::revert_hunk(Submission::RevertHunk { path: "src/a.rs".into(), to_turn: 2, hunk: 1, now_digest: content_digest(b"now") })]
    #[case::background(Submission::Background { call_id: CallId::new() })]
    #[case::user_shell(Submission::UserShell { command: "ls".into(), share: true })]
    #[case::user_agent(Submission::UserAgent { name: "explore".into(), task: "find the router".into() })]
    #[case::redo(Submission::Redo)]
    #[case::shutdown(Submission::Shutdown)]
    #[case::rename(Submission::Rename { title: "Fix the ledger".into() })]
    #[case::task_message(Submission::TaskMessage { task: TaskId::new(), from: None, hop: 0, text: "follow up".into() })]
    fn submission_json_roundtrip(#[case] submission: Submission) {
        let json = serde_json::to_string(&submission).expect("serialize");
        let back: Submission = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(submission, back);
    }

    /// T34.4: the wire shape SM§1 specifies, both directions and both
    /// `from` cases (parent/user vs. a sibling `TaskId`), survive a JSON
    /// round trip byte-for-byte in the fields that matter.
    #[test]
    fn task_message_round_trips_through_serde() {
        let sub = Submission::TaskMessage {
            task: TaskId::new(),
            from: None,
            hop: 0,
            text: "from the parent".into(),
        };
        let json = serde_json::to_string(&sub).expect("serialize");
        let back: Submission = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(sub, back);

        let event = Event::TaskMessage {
            task: TaskId::new(),
            from: Some(TaskId::new()),
            hop: 2,
            text: "from a sibling".into(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let back: Event = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, back);
    }

    /// plan.md T0.2 step 4: grep the serialized form for the `type` tag and
    /// assert it is snake_case, over one value per `Event` variant.
    #[rstest]
    #[case::session_started(Event::SessionStarted { session: SessionId::new(), config_digest: "d".into(), cwd: PathBuf::from(".") })]
    #[case::turn_started(Event::TurnStarted { turn: TurnId::new(), seq: 1, job: Job::Main, tier: Tier::Code, model: ModelId("m".into()) })]
    #[case::rewound(Event::Rewound { to_turn: 2, code: true, conversation: false, restored: vec![PathBuf::from("a.rs")], skipped: vec![] })]
    #[case::tool_call_done(Event::ToolCallDone { call_id: CallId::new(), result: ToolResult { ok: true, visible: "ok".into(), archive: None, bytes: 0, duration_ms: 0, diff: None, structured: None } })]
    #[case::model_switched(Event::ModelSwitched { tier: Tier::Cheap, from: ModelId("a".into()), to: ModelId("b".into()) })]
    #[case::state_changed(Event::StateChanged { mode: PermissionMode::Default, effort: None })]
    #[case::mode_changed(Event::ModeChanged { mode: Mode::Editor, permission_mode: PermissionMode::Default })]
    #[case::question_asked(Event::QuestionAsked { call_id: CallId::new(), question: "q".into(), options: vec![], source: None })]
    #[case::title_set(Event::TitleSet { title: "t".into(), by_user: true })]
    fn event_tags_are_snake_case(#[case] event: Event) {
        let json = serde_json::to_value(&event).expect("serialize");
        let tag = json
            .get("type")
            .and_then(|v| v.as_str())
            .expect("has type tag");
        assert!(
            tag.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
            "tag {tag:?} is not snake_case"
        );
    }

    #[test]
    fn tool_spec_schema_generates() {
        #[derive(JsonSchema)]
        #[allow(dead_code)]
        struct ReadInput {
            path: String,
            lines: Option<String>,
        }

        let schema = schemars::schema_for!(ReadInput);
        let schema_value = serde_json::to_value(&schema).expect("schema serializes");
        let spec = ToolSpec {
            name: "read".into(),
            description: "Read a file".into(),
            input_schema: schema_value.clone(),
            deferred: false,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        };
        assert_eq!(spec.input_schema, schema_value);
        assert!(schema_value.get("properties").is_some());
    }

    #[test]
    fn tool_result_from_an_old_rollout_loads_without_structured() {
        let old = serde_json::json!({
            "ok": true, "visible": "[ ] 1: a", "archive": null,
            "bytes": 8, "duration_ms": 0, "diff": null
        });
        let result: ToolResult = serde_json::from_value(old).expect("old shape loads");
        assert_eq!(result.structured, None);
        assert_eq!(result.todo_list(), None);
    }

    #[test]
    fn todo_list_reads_the_structured_payload() {
        let result = ToolResult {
            ok: true,
            visible: String::new(),
            archive: None,
            bytes: 0,
            duration_ms: 0,
            diff: None,
            structured: Some(Box::new(serde_json::json!([
                {"id": "1", "text": "a", "state": "in_progress"}
            ]))),
        };
        assert_eq!(
            result.todo_list(),
            Some(vec![TodoItem {
                id: "1".into(),
                text: "a".into(),
                state: TodoState::InProgress,
            }])
        );
    }

    /// A113: a `TitleSet` written before T37.22.9 has no `by_user` and
    /// reads as a generated title.
    #[test]
    fn title_set_without_by_user_reads_as_generated() {
        let old = r#"{"type":"title_set","title":"Fix the ledger"}"#;
        assert_eq!(
            serde_json::from_str::<Event>(old).expect("old rollout line"),
            Event::TitleSet {
                title: "Fix the ledger".into(),
                by_user: false
            }
        );
    }
}
