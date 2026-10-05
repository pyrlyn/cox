// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `Submission`, `Event`, `Item`, config and tool-schema types: every type
//! that crosses a crate boundary. Kept separate so every other crate can
//! depend on the contract without depending on any implementation.
//!
//! `cox-protocol` has no logic beyond serde: the turn loop lives in
//! `cox-core`, the wire formats live in `cox-provider`, the sandboxed tool
//! implementations live in `cox-tools`. What lives here is what
//! `cox-core` depends on instead of depending on those crates directly
//! (AGENTS.md: "anything that talks to the network, the filesystem or a
//! process lives behind a trait in `cox-protocol`").
//!
//! - [`ids`] — ULID newtypes (`SessionId`, `TurnId`, `ItemId`, `CallId`, `ArchiveId`, `TaskId`).
//! - [`agent`] — `AgentDef`, `tier_for`: a discovered subagent definition. Not reachable from `Submission`/`Event` (no wire schema), but crosses the `cox-ext` → `cox-core` boundary, so it lives here (T34.1).
//! - [`types`] — `Submission`, `Event`, the provider-neutral `Request`/`Content`, `ToolSpec`, and everything reachable from them.
//! - [`errors`] — the error taxonomy (plan.md §1.14): `ProviderError`, `ToolError`, `CoreError`, `StoreError`, `ExtError`, `McpError`.
//! - [`traits`] — `Provider`, `Tool`, `ToolCx`, `Store`, `Hook`, `Archive`, `PluginStore`, `Relay`: the seams every other crate implements against.
//! - [`image`] — `sniff`, the size cap and base64 encoding for images: the one check an image passes before it becomes an `Attachment` or a tool-output payload (T40.1).
//! - [`config`] — the `Config` struct tree mirroring `config/default.toml` (plan.md §1.6).
//! - [`plugin`] — `cox-plugin-api` re-exported (A52): the `plugin.toml` manifest and, later, the ABI payloads. It lives in its own crate because the guest SDK builds it for wasm32.

pub mod agent;
pub mod commands;
pub mod config;
pub mod errors;
pub mod ids;
pub mod image;
pub mod traits;
pub mod types;

pub use cox_plugin_api as plugin;

pub use config::{Config, DEFAULT_CONFIG_TOML};
pub use errors::{
    CoreError, ExtError, McpError, ProviderError, StoreError, ToolError, WorktreeError,
};
pub use ids::{ArchiveId, CallId, ItemId, SessionId, TaskId, TurnId};
pub use traits::{
    Archive, ArchivePut, Before, Change, CheckpointRow, Checkpointer, GrantScope, Hook,
    HunkReverter, MemoryHit, PluginGrant, PluginStore, PreImage, Provider, Relay, SessionRow,
    Snapshot, Store, Tool, ToolCx, UsageRow,
};
pub use types::ArchiveRef;
pub use types::{
    ApprovalPolicy, Attachment, Caps, CheckpointFile, CheckpointKind, Concurrency, Content,
    ContextBreakdown, DecidedBy, Decision, Diff, Effort, Event, HookEvent, HookOutcome, Item,
    ItemKind, Job, Level, LinuxBackend, Message, ModelId, PermissionMode, ProviderEvent,
    ProviderId, Request, Risk, Role, SandboxMode, SandboxPolicy, Segments, SkipReason, SkippedFile,
    SlashCommand, StopReason, Submission, SystemBlock, Thinking, Tier, TodoItem, TodoState,
    ToolCall, ToolOutput, ToolResult, ToolSpec, Usage, Why,
};

#[cfg(test)]
mod tests {
    use std::path::Path;

    use schemars::{Schema, schema_for};
    use serde::Serialize;

    use crate::types::{Event, Submission};

    /// A stable schema for both the `docs/` consumer and the JSONL rollout
    /// reader in `cox-store`: generates a schemars JSON Schema covering
    /// `Event` and `Submission` and checks it against the committed
    /// `docs/protocol.jsonschema`, so a shape change is a reviewable diff
    /// instead of a silent break.
    #[derive(Serialize)]
    struct ProtocolSchema {
        event: Schema,
        submission: Schema,
    }

    #[test]
    fn protocol_jsonschema_matches_committed_file() {
        let schema = ProtocolSchema {
            event: schema_for!(Event),
            submission: schema_for!(Submission),
        };
        let generated = serde_json::to_string_pretty(&schema).expect("schema serializes") + "\n";

        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/protocol.jsonschema");
        match std::fs::read_to_string(&path) {
            Ok(committed) => assert_eq!(
                committed, generated,
                "docs/protocol.jsonschema is stale; regenerate it (see this test) and commit it"
            ),
            Err(_) => {
                // First run: create it. `git status` will show it as new/changed for review.
                std::fs::write(&path, &generated).expect("write docs/protocol.jsonschema");
            }
        }
    }

    /// Smoke test that the crate's public re-exports actually round-trip
    /// through serde end to end, independent of the per-variant tests in
    /// `types.rs`/`errors.rs`.
    #[test]
    fn lib_reexports_are_usable() {
        let ev = Event::TurnDone {
            turn: crate::ids::TurnId::new(),
            stop: crate::types::StopReason::EndTurn,
        };
        let json = serde_json::to_value(&ev).expect("serialize");
        assert_eq!(json.get("type").and_then(|v| v.as_str()), Some("turn_done"));
    }
}
