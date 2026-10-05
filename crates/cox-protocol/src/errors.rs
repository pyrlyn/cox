// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Error taxonomy (plan.md §1.14), one enum per crate that can fail.
//! Every variant is `thiserror` (for `Display`/`std::error::Error`) and
//! `Clone + Serialize + Deserialize`, because `CoreError` rides inside
//! `Event::Error` and must survive the JSONL rollout round trip. Wrapping a
//! foreign error (`std::io::Error`, `rusqlite::Error`, …) always collapses
//! it to a bare variant or a `String` message at the boundary that produced
//! it — those foreign types are not `Clone`/`Serialize` and cox never
//! panics to avoid the conversion.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::CallId;

/// Failures from a `Provider` implementation (`cox-provider`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderError {
    /// The provider rejected the request's credentials.
    #[error("provider auth failed")]
    Auth,
    /// The provider is rate-limiting this key.
    #[error("rate limited{}", .retry_after.map(|s| format!(", retry after {s}s")).unwrap_or_default())]
    RateLimited {
        /// Seconds to wait before retrying, from the provider's `retry-after`, if given.
        retry_after: Option<u64>,
    },
    /// The provider is temporarily overloaded (5xx, no retry-after).
    #[error("provider overloaded")]
    Overloaded,
    /// The provider rejected the request shape itself (4xx, not auth).
    #[error("bad request: {message}")]
    BadRequest {
        /// The provider's error message, verbatim.
        message: String,
    },
    /// The assembled request exceeds the model's context window.
    #[error("context too long: {got} tokens > {max} max")]
    ContextTooLong {
        /// The model's max context, in tokens.
        max: u32,
        /// The estimated size of the request that was rejected.
        got: u32,
    },
    /// The model declined to continue (content policy, not an error).
    #[error("refusal: {detail}")]
    Refusal {
        /// The provider's refusal text, if any.
        detail: String,
    },
    /// A transport-level failure (DNS, TLS, connection reset).
    #[error("network error")]
    Network,
    /// The request exceeded `providers.*.timeout_s`.
    #[error("provider timed out")]
    Timeout,
    /// The stream was cancelled via `Interrupt`.
    #[error("provider call cancelled")]
    Cancelled,
    /// The SSE/JSON stream contained a line cox could not parse.
    #[error("parse error at line {line}")]
    Parse {
        /// 1-based line number within the stream where parsing failed.
        line: u64,
    },
    /// The request used a capability (`thinking`, `cache`, …) the provider lacks.
    #[error("unsupported feature: {feature}")]
    Unsupported {
        /// The capability name, matching a `Caps` field.
        feature: String,
    },
}

/// Failures from a `Tool` implementation (`cox-tools`, `cox-mcp`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolError {
    /// The permission engine denied the call before it ran.
    #[error("denied: {why}")]
    Denied {
        /// Human-readable reason, shown to the model so it can try another approach.
        why: String,
    },
    /// A path argument escaped every workspace root.
    #[error("path {path:?} escapes workspace root {root:?}")]
    Confined {
        /// The path the tool was given.
        path: PathBuf,
        /// The workspace root it failed to stay under.
        root: PathBuf,
    },
    /// The sandbox refused the operation.
    #[error("sandbox denied: {detail}")]
    SandboxDenied {
        /// The sandbox backend's denial detail.
        detail: String,
    },
    /// The tool exceeded its allotted time.
    #[error("tool timed out")]
    Timeout,
    /// The target (file, path, id) does not exist.
    #[error("not found")]
    NotFound,
    /// An `edit` match was not unique.
    #[error("ambiguous match: {} candidates", .matches.len())]
    Ambiguous {
        /// The matching line numbers or excerpts, for the model to disambiguate.
        matches: Vec<String>,
    },
    /// The output exceeded the archive/visible cap.
    #[error("too large: {bytes} bytes > {cap} cap")]
    TooLarge {
        /// The actual size in bytes.
        bytes: u64,
        /// The configured cap in bytes.
        cap: u64,
    },
    /// The target file is binary; the tool refuses to read/edit it as text.
    #[error("binary file")]
    Binary,
    /// A filesystem I/O error occurred.
    #[error("io error")]
    Io,
    /// The call was cancelled via the shared `CancellationToken`.
    #[error("cancelled")]
    Cancelled,
}

/// Failures from `cox-core`'s turn loop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CoreError {
    /// The session or monthly budget cap was hit.
    #[error("budget exceeded: spent ${spent:.2} of ${cap:.2}")]
    Budget {
        /// Amount spent so far, in USD.
        spent: f64,
        /// The configured cap, in USD.
        cap: f64,
    },
    /// The turn was cancelled via `Submission::Interrupt`.
    #[error("interrupted")]
    Interrupted,
    /// The provider call failed.
    #[error("provider error: {error}")]
    Provider {
        /// The underlying provider failure.
        error: ProviderError,
    },
    /// A tool call failed.
    #[error("tool call {call} failed: {error}")]
    Tool {
        /// The call that failed.
        call: CallId,
        /// The underlying tool failure.
        error: ToolError,
    },
    /// Compaction failed to produce a summary.
    #[error("compaction failed")]
    Compaction,
    /// A config value was invalid or out of range.
    #[error("config error at {key}: {message}")]
    Config {
        /// The dotted config key.
        key: String,
        /// Why the value was rejected.
        message: String,
    },
    /// The store failed (fatal to the session, per plan.md §1.14).
    #[error("store error: {error}")]
    Store {
        /// The underlying store failure.
        error: StoreError,
    },
    /// A hook failed in a way that was not fail-open (rare; hooks usually just warn).
    #[error("hook {id} error: {error}")]
    Hook {
        /// The hook's configured id.
        id: String,
        /// The hook's error text.
        error: String,
    },
    /// A subagent action was refused by a named guard — T34.2's
    /// concurrency cap or T34.6's message-flood cap — shaped like
    /// `ToolError::Denied` so `Relay::send_message` can hand the same text
    /// back to whoever called it.
    #[error("denied: {why}")]
    Denied {
        /// Human-readable reason.
        why: String,
    },
    /// An external agent process (EA§5, e.g. a plugin-driven CLI like
    /// Cursor's) reported its own run as failed. Distinct from `Provider`
    /// (A58): the external agent stands in for a whole child session, not
    /// cox's own model call, so its failure must never be classified or
    /// retried as one — `cox-provider-http::retry::retryable` only ever
    /// takes a `ProviderError`, which this variant never becomes.
    #[error("external agent {agent} failed: {message}")]
    ExternalAgent {
        /// The external agent's name (its plugin/preset id).
        agent: String,
        /// The agent's own error text, already sanitized by the caller.
        message: String,
    },
}

/// Failures from `cox-store`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StoreError {
    /// `cox.db` could not be opened.
    #[error("could not open store")]
    Open,
    /// A schema migration failed partway.
    #[error("migration failed: {from} -> {to}")]
    Migrate {
        /// The schema version migrated from.
        from: u32,
        /// The schema version migration was attempting to reach.
        to: u32,
    },
    /// The store file exists but failed an integrity check.
    #[error("corrupt store at {path:?}")]
    Corrupt {
        /// Path to the corrupt file.
        path: PathBuf,
    },
    /// The requested row does not exist.
    #[error("not found")]
    NotFound,
    /// A filesystem I/O error occurred.
    #[error("io error")]
    Io,
    /// The underlying SQLite call failed.
    #[error("sqlite error")]
    Sqlite,
    /// A plugin kv write went over quota (PL§3: 64 KiB per value, 1 MiB per
    /// plugin across all its keys).
    #[error("plugin kv quota exceeded")]
    QuotaExceeded,
    /// `cox.db` holds a migration this binary does not embed: a newer `cox`
    /// (the app's bundled one, or a Homebrew one of another version) wrote
    /// it, and this older one must not read or write it (T37.36).
    #[error(
        "cox.db was migrated by a newer cox (schema {db}; this cox knows up to {binary}): \
         update this cox (e.g. `brew upgrade cox`, or the app's own update) or run the newer one"
    )]
    SchemaNewer {
        /// The newest migration version in `cox.db` this binary does not know.
        db: String,
        /// The newest migration version this binary embeds.
        binary: String,
    },
}

/// Failures from `cox-ext` (instruction files, skills, hooks, commands).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExtError {
    /// A `SKILL.md`/command frontmatter block failed to parse.
    #[error("bad frontmatter in {path:?} at line {line}")]
    Frontmatter {
        /// The file with the bad frontmatter.
        path: PathBuf,
        /// 1-based line number of the offending YAML.
        line: u64,
    },
    /// A hook process exceeded `hooks.timeout_s`.
    #[error("hook timed out")]
    HookTimeout,
    /// A hook process exited non-zero unexpectedly.
    #[error("hook crashed with status {status}")]
    HookCrashed {
        /// The process exit status.
        status: i32,
    },
    /// An instruction file exceeded its token budget.
    #[error("{path:?} exceeds budget of {budget} tokens")]
    TooLarge {
        /// The oversized file.
        path: PathBuf,
        /// The configured token budget it exceeded.
        budget: u64,
    },
    /// An instruction/skill file `@`-imports itself, directly or transitively.
    #[error("import cycle at {path:?}")]
    Cycle {
        /// The file where the cycle was detected.
        path: PathBuf,
    },
}

/// Failures from `cox-mcp`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpError {
    /// The server process could not be spawned (stdio transport).
    #[error("could not spawn mcp server")]
    Spawn,
    /// The MCP `initialize` handshake failed.
    #[error("mcp handshake failed")]
    Handshake,
    /// OAuth or credential exchange failed.
    #[error("mcp auth failed")]
    Auth,
    /// The server exceeded `mcp.timeout_s`.
    #[error("mcp call timed out")]
    Timeout,
    /// The stdio/HTTP transport broke mid-session.
    #[error("mcp transport error")]
    Transport,
    /// The server's tool call itself returned an error result.
    #[error("mcp tool {server}/{tool} failed")]
    ToolFailed {
        /// The server's configured name.
        server: String,
        /// The tool name within that server.
        tool: String,
    },
}

/// Failures of the worktree helper (T27.3): what `cox --worktree` and
/// `agent(isolation: "worktree")` report when git refuses or a rule of the
/// `worktrees` skill applies. The owner and dirty-tree refusals are rules,
/// not git errors, so they are variants rather than one message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorktreeError {
    /// The name is not `[a-z0-9][a-z0-9._-]*` after lower-casing.
    #[error("worktree name {name:?} may only contain a-z 0-9 . _ -")]
    BadName {
        /// What was asked for.
        name: String,
    },
    /// `dir` is not inside a git repository.
    #[error("{dir:?} is not inside a git repository")]
    NotARepository {
        /// The directory the lookup started from.
        dir: PathBuf,
    },
    /// The path is not one of the repository's worktrees.
    #[error("{path:?} is not a registered worktree")]
    NotRegistered {
        /// The path asked about.
        path: PathBuf,
    },
    /// A registered path belongs to another branch (or is detached).
    #[error("{path:?} is on branch {actual:?}, expected {expected:?}")]
    BranchMismatch {
        /// The registered worktree path.
        path: PathBuf,
        /// The requested branch.
        expected: String,
        /// The branch Git reported; `None` means detached HEAD.
        actual: Option<String>,
    },
    /// The lock reason names someone else; leave it and tell the creator.
    #[error("{path:?} is locked by another owner ({reason})")]
    LockedByOther {
        /// The worktree.
        path: PathBuf,
        /// The lock reason as `git worktree list` shows it.
        reason: String,
    },
    /// Uncommitted or untracked files: commit them or ask the creator.
    #[error("{path:?} has uncommitted or untracked files")]
    Dirty {
        /// The worktree.
        path: PathBuf,
    },
    /// Removing the main checkout is never what anyone meant.
    #[error("refusing to remove the main checkout {path:?}")]
    MainCheckout {
        /// The main checkout.
        path: PathBuf,
    },
    /// A git command failed; `stderr` is git's text.
    #[error("git {args}: {stderr}")]
    Git {
        /// The arguments, space-joined.
        args: String,
        /// What git said, trimmed.
        stderr: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn provider_error_json_roundtrip() {
        let err = ProviderError::RateLimited {
            retry_after: Some(30),
        };
        let json = serde_json::to_string(&err).expect("serialize");
        assert_eq!(json, r#"{"type":"rate_limited","retry_after":30}"#);
        let back: ProviderError = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(err, back);
    }

    #[test]
    fn core_error_wraps_provider_and_tool_errors() {
        let err = CoreError::Tool {
            call: CallId::new(),
            error: ToolError::Timeout,
        };
        let json = serde_json::to_string(&err).expect("serialize");
        let back: CoreError = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(err, back);
    }

    #[test]
    fn error_tags_are_snake_case() {
        let err = ExtError::HookCrashed { status: 1 };
        let json = serde_json::to_value(&err).expect("serialize");
        let tag = json.get("type").and_then(|v| v.as_str()).expect("type tag");
        assert_eq!(tag, "hook_crashed");
    }
}
