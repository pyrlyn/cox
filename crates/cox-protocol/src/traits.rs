// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The traits every other crate implements against instead of a concrete
//! type (plan.md §1.2). This is the enforcement point for AGENTS.md's rule
//! that anything touching the network, filesystem or a process lives
//! behind a trait defined here: `cox-core` depends only on these
//! signatures, never on `cox-provider`/`cox-tools`/`cox-mcp`/`cox-store`.
//! `Relay` (T34.6, SM§4) is the same shape for `send_message`: `cox-tools`
//! needs no `Session` handle, only this narrow hook back into it.
//! `ExternalAgent` (T35.5, EA§3) is the process an external-agent preset
//! runs, spawned by the host so `cox-core` stays I/O-free.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::errors::{ProviderError, StoreError, ToolError, WorktreeError};
use crate::ids::{ArchiveId, CallId, SessionId};
use crate::types::{
    Caps, CheckpointKind, ProviderEvent, ProviderId, Request, Risk, SandboxPolicy, ToolOutput,
    ToolSpec, Usage,
};

/// A row inserted for a new session (`Store::session_create`), matching the
/// `sessions` table (plan.md §1.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRow {
    /// The session's id.
    pub id: SessionId,
    /// RFC 3339 creation timestamp.
    pub created_at: String,
    /// The working directory the session started in.
    pub cwd: PathBuf,
    /// The project slug used for memory/config lookup.
    pub project_slug: String,
    /// The session's title, if one has been generated.
    pub title: Option<String>,
    /// The parent session, for subagents.
    pub parent_id: Option<SessionId>,
    /// Where the JSONL rollout for this session lives.
    pub rollout_path: PathBuf,
}

/// A row inserted for one provider call (`Store::usage_insert`), matching
/// the `usage` table (plan.md §1.7). "A cost that is not a `usage` row in
/// the ledger does not exist" — AGENTS.md.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageRow {
    /// The session this call belongs to.
    pub session_id: SessionId,
    /// Turn number within the session.
    pub turn: u32,
    /// The job this call served.
    pub job: crate::types::Job,
    /// The tier routed to.
    pub tier: crate::types::Tier,
    /// Which provider backend served the call.
    pub provider: ProviderId,
    /// The specific model used.
    pub model: crate::types::ModelId,
    /// The reasoning effort the router settled on, after clamping to what
    /// the model supports. `None` only for rows written before the ledger
    /// recorded effort — never a stand-in for "default".
    pub effort: Option<crate::types::Effort>,
    /// The recorded usage (tokens, cost, latency).
    pub usage: Usage,
}

/// What `Store::archive_put`/`Archive::put` writes: the full, untruncated
/// bytes for one tool call's output, before the model ever sees the
/// truncated form (plan.md §1.7/D6a).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArchivePut {
    /// The session the call ran in.
    pub session: SessionId,
    /// The call being archived.
    pub call: CallId,
    /// The tool name.
    pub tool: String,
    /// The call's subject (path, command line, URL), if any.
    pub subject: Option<String>,
    /// The full output bytes.
    pub bytes: Vec<u8>,
}

/// One memory search hit (`Store::memory_search`), backed by the
/// `memory_fts` virtual table (plan.md §1.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryHit {
    /// The memory fact's name.
    pub name: String,
    /// Where it lives under `projects/<slug>/memory/`.
    pub path: PathBuf,
    /// A short excerpt around the match.
    pub snippet: String,
}

/// One `checkpoints` row (`Store::checkpoint_insert`, T26.1): what a tool
/// call was about to change, archived before it ran, or the marker that
/// starts a user turn. `/rewind` walks these newest-first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointRow {
    /// The session the change belongs to.
    pub session: SessionId,
    /// The turn number within the session (the FTS/ledger turn counter).
    pub turn: u32,
    /// The call that made the change; `None` for a turn marker or a rewind.
    pub call: Option<CallId>,
    /// The confined absolute path; empty for a turn marker.
    pub path: PathBuf,
    /// What was recorded.
    pub kind: CheckpointKind,
    /// Where the pre-image bytes live; `None` for `Created`/`Turn` and for a
    /// file too large to archive (recorded so `/rewind` can say so).
    pub archive: Option<ArchiveId>,
}

/// What a file held before a call touched it. Three states, not an
/// `Option`: a file too large to keep must not be mistaken for one that did
/// not exist, or a rewind would delete it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Before {
    /// No file at that path.
    Absent,
    /// The full bytes.
    Bytes(Vec<u8>),
    /// A file over the implementation's size cap; only its existence is kept.
    TooLarge,
}

/// A file's state before a call that names its path (`Checkpointer::preimages`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreImage {
    /// The confined absolute path.
    pub path: PathBuf,
    /// What was there.
    pub before: Before,
}

/// An opaque fingerprint of the workspace roots (`Checkpointer::snapshot`):
/// one tree id per root. Only the implementation that made it reads it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    /// `(root, tree id)` pairs in root order.
    pub trees: Vec<(PathBuf, String)>,
}

/// One file that differs between two snapshots (`Checkpointer::changes`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The confined absolute path.
    pub path: PathBuf,
    /// The file is gone in the later snapshot.
    pub deleted: bool,
    /// What the earlier snapshot held (`Absent` for a created file).
    pub before: Before,
}

/// A model provider: turns a `Request` into a stream of `ProviderEvent`s.
/// Implemented by `cox-provider` for Anthropic/OpenAI/local backends and by
/// `Scripted`/`Replay` fakes for tests (D12); `cox-core` never depends on
/// which.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Which provider this is.
    fn id(&self) -> ProviderId;
    /// What this provider implementation can do, so `cox-core` can avoid
    /// sending it a request shape it does not support.
    fn capabilities(&self) -> Caps;
    /// Whether `model` on this wire takes `Content::Image` in a user
    /// message (T37.6). Per model because one Chat server hosts vision and
    /// text-only models side by side; `false` by default so a wire that
    /// never said so gets a notice instead of an image it would reject.
    fn accepts_images(&self, model: &str) -> bool {
        let _ = model;
        false
    }
    /// Streams a response, forwarding `ProviderEvent`s on `sink` as they
    /// arrive; returns the call's final `Usage` once the stream ends, or
    /// stops early if `cancel` fires.
    async fn stream(
        &self,
        req: Request,
        sink: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<Usage, ProviderError>;
    /// Counts tokens for a request without sending it, when the provider supports it.
    async fn count_tokens(&self, req: &Request) -> Result<u32, ProviderError>;
}

/// Context handed to a running `Tool::call`: everything it needs that is
/// not part of its own input, and nothing it could use to bypass a trust
/// boundary (AGENTS.md) — paths still go through `cox_tools::path::confine`,
/// commands still go through `cox_tools::sandbox`.
pub struct ToolCx {
    /// Workspace roots the call may read within.
    pub roots: Vec<PathBuf>,
    /// Workspace roots the call may mutate. Normally identical to `roots`;
    /// isolated worktrees omit the main checkout from this set.
    pub writable_roots: Vec<PathBuf>,
    /// The call's working directory.
    pub cwd: PathBuf,
    /// The sandbox policy in effect for this call.
    pub sandbox: SandboxPolicy,
    /// Where to archive the call's full output before truncation.
    pub archive: Arc<dyn Archive>,
    /// Shared cancellation for `Submission::Interrupt`.
    pub cancel: CancellationToken,
    /// A channel for streaming partial output (`Event::ToolCallOutput`).
    pub output: mpsc::Sender<String>,
    /// The session this call belongs to.
    pub session: SessionId,
    /// This call's id.
    pub call: CallId,
    /// The subagent name this session runs as (`explore-2`, T27.2), set
    /// once by `Session::spawn_child`; `None` for the session the user is
    /// talking to. `ask_user` (T34.3) turns this into the same `Source`
    /// `relay_approval` already builds for a relayed approval.
    pub agent: Option<String>,
    /// The dispatched preset/def name (`explore`), alongside `agent`.
    pub preset: Option<String>,
    /// `send_message`'s only way into a session (T34.6), same pattern as
    /// `agent`/`preset` (T34.3): the session that builds this `ToolCx`
    /// (`cox-core/src/turn.rs`) fills it with itself, so a child session's
    /// call reaches the child's own `Relay` impl (its `self_task`, never
    /// the top-level session's) and the parent's reaches the parent's.
    /// `None` where no session builds one (a unit test's bare `ToolCx`).
    pub relay: Option<Arc<dyn Relay>>,
}

/// A built-in or MCP tool. Implemented by `cox-tools` (`read`, `edit`,
/// `bash`, …) and `cox-mcp` (one per discovered server tool).
#[async_trait]
pub trait Tool: Send + Sync {
    /// The tool's advertised name, schema, risk and concurrency.
    fn spec(&self) -> ToolSpec;
    /// What permission rules match this call on: the confined path, command
    /// line, URL, or namespaced MCP name.
    fn subject(&self, input: &Value) -> String;
    /// The commands a shell subject splits into, so a rule covers each one
    /// rather than the line's first word (T36.1). `None` — every tool that
    /// is not a shell — keeps the subject as one unit.
    fn segments(&self, _input: &Value) -> Option<crate::types::Segments> {
        None
    }
    /// This *call's* risk, which is not always the tool's. `spec().risk` is
    /// a default: `apply_patch` is an ordinary write until the patch in
    /// front of it deletes more than five files, and only the input says
    /// which it is (plan.md §4 tool table, T3.5 step 4).
    fn risk(&self, _input: &Value) -> Risk {
        self.spec().risk
    }
    /// The paths this call writes, when its input names them (`edit`,
    /// `write`, `apply_patch`). `None` means "unknown" — `bash`, MCP tools —
    /// and the loop snapshots the whole workspace around the call instead
    /// (T26.1). Read-only tools are never asked.
    fn touches(&self, _input: &Value) -> Option<Vec<String>> {
        None
    }
    /// Runs the tool. `text` in the returned `ToolOutput` is untruncated;
    /// the core archives it and truncates what the model sees.
    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError>;
    /// Releases what the tool holds across calls — a process such as a
    /// language server (T41.5). Runs once, from `Session::end` of the
    /// session that owns the tool list, never from a child that shares it.
    /// Sync because `end` is: a tool that owns a process kills it here
    /// rather than awaiting a polite exit. A tool must still work after
    /// it, starting again on its next call, since a surface may reuse its
    /// tool list for a new session. Default: nothing to release.
    fn shutdown(&self) {}
}

/// The persistence layer: `~/.cox/cox.db` plus the JSONL rollouts
/// (plan.md §1.7). Sync on purpose (D9): hooks, tests and `cox stats` need
/// no async runtime to query it.
pub trait Store: Send + Sync {
    /// Opens (creating and migrating if needed) the store under `home`.
    fn open(home: &Path) -> Result<Self, StoreError>
    where
        Self: Sized;
    /// Records a new session.
    fn session_create(&self, s: &SessionRow) -> Result<(), StoreError>;
    /// Appends one event to a session's rollout, returning its sequence number.
    fn rollout_append(&self, id: &SessionId, ev: &crate::types::Event) -> Result<u64, StoreError>;
    /// Reads back a session's full rollout, in order.
    fn rollout_read(&self, id: &SessionId) -> Result<Vec<crate::types::Event>, StoreError>;
    /// Records one provider call's usage/cost.
    fn usage_insert(&self, row: &UsageRow) -> Result<(), StoreError>;
    /// Archives a tool call's full output, returning its id.
    fn archive_put(&self, a: &ArchivePut) -> Result<ArchiveId, StoreError>;
    /// Reads back archived bytes by id (`cox expand <id>`).
    fn archive_get(&self, id: &ArchiveId) -> Result<Vec<u8>, StoreError>;
    /// Full-text searches memory facts for a project.
    fn memory_search(&self, q: &str, limit: usize) -> Result<Vec<MemoryHit>, StoreError>;
    /// Records one memory fact's searchable text (plan.md T10.1): the
    /// `memory` row and its `memory_fts` row share a rowid so
    /// `memory_search`'s join lines up. Re-saving a name replaces both rows.
    fn memory_upsert(
        &self,
        project: &str,
        name: &str,
        path: &str,
        kind: &str,
        body: &str,
    ) -> Result<(), StoreError>;
    /// Indexes one model-visible text for session search (plan.md T10.3).
    /// Best-effort by contract: the rollout is the source of truth and the
    /// loop ignores failures, so a broken index degrades search, never turns.
    fn rollout_index(&self, session: &SessionId, turn: u32, text: &str) -> Result<(), StoreError>;
    /// Records one checkpoint row (T26.1). Written before the matching
    /// `Event::Checkpoint` is emitted, never after.
    fn checkpoint_insert(&self, row: &CheckpointRow) -> Result<(), StoreError>;
    /// Every checkpoint row of a session in insertion order.
    fn checkpoint_list(&self, session: &SessionId) -> Result<Vec<CheckpointRow>, StoreError>;
}

/// Where a plugin grant applies (PL§3): the whole user install, or one
/// repository. A project plugin's grant is additionally keyed on the
/// repository root, since a repository must not silently gain the
/// capabilities the user already granted elsewhere (PL§1: "cloning a
/// repository never runs its plugins").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GrantScope {
    /// `~/.cox/plugins/<id>`: applies across every repository.
    User,
    /// `<git root>/.cox/plugins/<id>`: applies only to that repository.
    Project(PathBuf),
}

/// One `plugin_grants` row (PL§3): what capabilities were approved for a
/// plugin id at a package digest, in a scope. `capabilities` and `source`
/// are opaque JSON here — the granted-capability shape and the install
/// source (`{kind: "path", path, digest}`, PL§1a) are `cox-plugin`'s to
/// define; `cox-store` only persists and returns them unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginGrant {
    /// The plugin's manifest id.
    pub plugin_id: String,
    /// Where this grant applies.
    pub scope: GrantScope,
    /// The package digest this grant was decided against. Changed bytes
    /// mean a different row, not an update of this one (PL§3).
    pub digest: String,
    /// The capabilities the user approved, as `cox-plugin` shapes them.
    pub capabilities: Value,
    /// Whether the plugin is currently enabled under this grant.
    pub enabled: bool,
    /// Where the plugin came from (`cox plugin install`'s source record).
    pub source: Value,
    /// RFC 3339 timestamp of the decision.
    pub decided_at: String,
}

/// `plugin_kv` per-value quota (PL§3); a `PluginStore::kv_put` over it is
/// `StoreError::QuotaExceeded`.
pub const KV_VALUE_LIMIT: usize = 64 * 1024;

/// `plugin_kv` per-plugin quota, summed across all its keys (PL§3).
pub const KV_PLUGIN_LIMIT: usize = 1024 * 1024;

/// Plugin grants and per-plugin key-value storage (PL§3, A52). Kept apart
/// from `Store` so approving or persisting plugin state does not grow the
/// trait every other surface implements; `cox-store`'s `Store` implements
/// this too. Sync for the same reason as `Store` (D9).
pub trait PluginStore: Send + Sync {
    /// The grant on file for this plugin id, scope and digest, if any
    /// decision was ever recorded for that exact key.
    fn grant_get(
        &self,
        plugin_id: &str,
        scope: &GrantScope,
        digest: &str,
    ) -> Result<Option<PluginGrant>, StoreError>;
    /// Inserts or replaces the grant at its `(plugin_id, scope, digest)` key.
    fn grant_put(&self, grant: &PluginGrant) -> Result<(), StoreError>;
    /// Flips `enabled` on an existing grant without touching its
    /// capabilities or digest (`cox plugin disable`).
    fn grant_set_enabled(
        &self,
        plugin_id: &str,
        scope: &GrantScope,
        digest: &str,
        enabled: bool,
    ) -> Result<(), StoreError>;
    /// Deletes every grant for a plugin id, across every scope and digest
    /// (`cox plugin remove`).
    fn grants_delete(&self, plugin_id: &str) -> Result<(), StoreError>;
    /// Reads one kv value.
    fn kv_get(&self, plugin_id: &str, key: &str) -> Result<Option<Vec<u8>>, StoreError>;
    /// Writes one kv value, rejecting it if the value or the plugin's total
    /// stored bytes would go over quota (64 KiB per value, 1 MiB per
    /// plugin, PL§3).
    fn kv_put(&self, plugin_id: &str, key: &str, value: &[u8]) -> Result<(), StoreError>;
    /// Deletes one kv value; deleting an absent key is not an error
    /// (`cox_kv_delete`, T33.9).
    fn kv_delete(&self, plugin_id: &str, key: &str) -> Result<(), StoreError>;
    /// Deletes every kv row for a plugin id (`cox plugin remove`).
    fn kv_delete_all(&self, plugin_id: &str) -> Result<(), StoreError>;
}

/// Where a tool's full, pre-truncation output is written before the model
/// sees the shortened form (D6a: "the archive row exists before the model
/// sees truncated text"). A narrower, async-friendly view of `Store`'s
/// archive methods, since `Tool::call` runs in an async context and `Store`
/// is deliberately sync (D9); `cox-store`'s `Store` implementation is also
/// the concrete `Archive`, dispatched onto a blocking task.
#[async_trait]
pub trait Archive: Send + Sync {
    /// Archives bytes, returning their id.
    async fn put(&self, put: ArchivePut) -> Result<ArchiveId, StoreError>;
    /// Reads back archived bytes by id.
    async fn get(&self, id: &ArchiveId) -> Result<Vec<u8>, StoreError>;
}

/// Puts one hunk of a file's net diff back (T51.19): cox-render's
/// `diffmodel::revert_hunk`, installed by the surface like the
/// `Checkpointer`, because cox-core may not depend on cox-render
/// (`deps.rs`). Pure: no file is read or written here.
pub trait HunkReverter: Send + Sync {
    /// `now` with hunk `index` of the line diff from `before` put back to
    /// `before`'s lines; `None` when the diff has no such hunk.
    fn revert(&self, before: &str, now: &str, index: usize) -> Option<String>;
}

/// Where the loop gets a file's bytes before a call changes it (T26.1).
/// Implemented by `cox-tools` (`checkpoint::GitCheckpointer`), which is
/// the crate allowed to read files and run git; `cox-core` only decides
/// *when* to ask and what to archive.
#[async_trait]
pub trait Checkpointer: Send + Sync {
    /// The current bytes of every path in `paths`, each confined to `roots`
    /// exactly as the tool will confine it. A path that fails confinement
    /// is skipped (the tool will refuse it too).
    async fn preimages(&self, roots: &[PathBuf], cwd: &Path, paths: &[String]) -> Vec<PreImage>;
    /// A fingerprint of everything under `roots` that is not ignored.
    async fn snapshot(&self, roots: &[PathBuf]) -> Result<Snapshot, ToolError>;
    /// What differs between two snapshots of the same roots, with the
    /// pre-image bytes of every modified or deleted file.
    async fn changes(&self, before: &Snapshot, after: &Snapshot) -> Result<Vec<Change>, ToolError>;
    /// Writes a pre-image back (`Some`) or removes a file `/rewind` undoes
    /// the creation of (`None`); the path is confined to `roots` first.
    async fn restore(
        &self,
        roots: &[PathBuf],
        cwd: &Path,
        path: &Path,
        bytes: Option<&[u8]>,
    ) -> Result<(), ToolError>;
}

/// A worktree a session or a subagent works in (T27.3), as
/// `Worktrees::add` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    /// The worktree's checkout: `<root>/_worktrees/<repo>-<name>`.
    pub path: PathBuf,
    /// The branch checked out there: `<name>`.
    pub branch: String,
    /// The main checkout the worktree belongs to.
    pub main: PathBuf,
}

/// One checkout of a repository as `Worktrees::list` reports it (T37.10):
/// what the desktop's worktree list shows next to its prune action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeInfo {
    /// The checkout's directory.
    pub path: PathBuf,
    /// The branch checked out there; `None` when detached.
    pub branch: Option<String>,
    /// The main checkout rather than a linked worktree.
    pub main: bool,
    /// The lock reason; `Some("")` when locked without one.
    pub locked: Option<String>,
    /// Git reports it prunable: its directory is gone.
    pub stale: bool,
    /// Its branch is already merged into the main checkout's `HEAD`.
    pub merged: bool,
    /// Bytes on disk under `path`, best effort.
    pub bytes: u64,
}

/// One file a worktree changed against the commit it was cut from, with
/// its line counts (T52.10): a best-of-n candidate's column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStat {
    /// Relative to the worktree.
    pub path: PathBuf,
    /// Lines added.
    pub added: u32,
    /// Lines removed.
    pub removed: u32,
}

/// Where the loop gets a worktree for `agent(isolation: "worktree")`
/// (T27.3). Implemented by `cox-tools` (`git::GitWorktrees`), the crate
/// allowed to run git; `cox-core` only decides which task gets one.
#[async_trait]
pub trait Worktrees: Send + Sync {
    /// The worktree named `name` of the repository around `from`, created
    /// per the workspace `worktrees` skill and locked for `owner`, or the
    /// existing one when it is already registered under a cox owner.
    async fn add(&self, from: &Path, name: &str, owner: &str) -> Result<Worktree, WorktreeError>;

    /// Every checkout of the repository around `from`, main first (T37.10).
    /// The default lists none: a source that only creates worktrees (a test
    /// fake) has nothing to report.
    async fn list(&self, from: &Path) -> Result<Vec<WorktreeInfo>, WorktreeError> {
        let _ = from;
        Ok(Vec::new())
    }

    /// What the worktree at `path` changed against the commit it was cut
    /// from, committed or not, untracked files included (T52.10). The
    /// default reports nothing.
    async fn diffstat(&self, path: &Path) -> Result<Vec<FileStat>, WorktreeError> {
        let _ = path;
        Ok(Vec::new())
    }

    /// Removes the worktree at `path`, which must be locked for `owner`
    /// (T52.10). A tree with uncommitted or untracked files is refused
    /// (`WorktreeError::Dirty`) unless `discard`, which the person gives
    /// only by confirming a second time that its changes go. The default
    /// removes nothing: a source that only creates worktrees has none of
    /// its own to remove.
    async fn remove(&self, path: &Path, owner: &str, discard: bool) -> Result<(), WorktreeError> {
        let _ = (owner, discard);
        Err(WorktreeError::NotRegistered {
            path: path.to_path_buf(),
        })
    }
}

/// Where the session gets its repo map (P43): a ranked outline of the
/// workspace at `root`, cut at `budget_bytes`, showing only files `admit`
/// accepts. Implemented over `cox-tools::repomap`, the crate allowed to walk
/// the tree and run git; `cox-core` decides when to build and passes the
/// permission engine as `admit`. Infallible: a map it cannot build is empty.
#[async_trait]
pub trait RepoMapper: Send + Sync {
    /// The map text, a pure function of the files, git order and budget.
    async fn build(
        &self,
        root: &Path,
        budget_bytes: usize,
        // An explicit `for<'p>`: `#[async_trait]` names every elided lifetime,
        // the one inside `Fn(&Path)` too, which would tie the argument to the
        // call and reject `admit(&root.join(rel))`.
        admit: &(dyn for<'p> Fn(&'p Path) -> bool + Send + Sync),
    ) -> String;
}

/// A hook source (`cox-ext`'s shell hooks, `cox-plugin`'s plugin hooks, or
/// a chain of them): reports its verdict for one hook event. Never returns
/// a `Result` — a broken hook is always a `HookOutcome::Failed`, never a
/// panic or a fatal error (D14/AGENTS.md: "fail open on extensions").
#[async_trait]
pub trait Hook: Send + Sync {
    /// Whether an observe-only trigger (`SessionStart`, `Notification`)
    /// should be dispatched to this source at all. The default is the
    /// `[hooks]` config check; a source configured elsewhere (a plugin's
    /// granted hooks, PL§6) answers for itself, or it would never run.
    fn interested(
        &self,
        event: crate::types::HookEvent,
        config: &crate::config::HooksConfig,
    ) -> bool {
        config.events.contains_key(event.name())
    }

    /// Runs the hook for `event` with `payload`, giving up after `timeout`.
    async fn run(
        &self,
        event: crate::types::HookEvent,
        payload: Value,
        timeout: Duration,
    ) -> crate::types::HookOutcome;
}

/// The session a tool talks back to: where `send_message` (T34.6, SM§4)
/// delivers a follow-up and where `ask_user` raises its question (DT G4).
/// Implemented by `Session` (`cox-core`) so `cox-tools` needs no handle to
/// it, only this narrow hook — the same shape as `Archive`/`Worktrees`
/// (AGENTS.md's trust-boundary rule: anything reaching outside this crate
/// goes through a trait defined here).
#[async_trait]
pub trait Relay: Send + Sync {
    /// Sends `text` to `to` (`"parent"`, a sibling's registry name, or a
    /// `TaskId`), stamped with the caller's own task if it is a subagent.
    async fn send_message(&self, to: &str, text: &str) -> Result<(), ToolError>;

    /// Emits `Event::QuestionAsked` and waits for the matching
    /// `Submission::Answer`; `Ok(None)` means the person dismissed it.
    async fn ask(
        &self,
        call_id: CallId,
        question: &str,
        options: &[String],
        source: Option<crate::types::Source>,
    ) -> Result<Option<String>, ToolError> {
        let _ = (call_id, question, options, source);
        Err(ToolError::Denied {
            why: "no surface is listening for questions".into(),
        })
    }
}

/// Where a session's events go besides its surface (PL§5, T33.10): the
/// plugin host's per-plugin rings. `Session::emit` calls it right after the
/// rollout append, with the scrubbed copy the rollout got and the sequence
/// number `rollout_append` returned, so all four surfaces feed plugins from
/// one place and in rollout order.
pub trait EventTap: Send + Sync {
    /// Takes one event. Must never wait on a plugin: a slow plugin loses
    /// events, it never slows a turn.
    fn offer(&self, seq: u64, ev: &crate::types::Event);
}

/// A granted `[[external_agents]]` entry's driver (EA§3, T35.5): another
/// vendor's CLI agent that `agent(preset: <name>)` dispatches in place of a
/// model. Implemented by the host, which spawns the CLI under the session's
/// `sandbox::Policy` (EA§2) and speaks ACP or stream-json per the
/// manifest's `mode`; `cox-core` only feeds it turns, so it never opens the
/// process itself.
#[async_trait]
pub trait ExternalAgent: Send + Sync {
    /// The dispatch name (`cursor`), also the usage row's model.
    fn name(&self) -> &str;
    /// Runs one turn for `prompt`, streaming the agent's work on `events`
    /// as cox events (`StreamJsonMapper` for stream-json); a `TurnDone` it
    /// sends ends the turn early. Returns the tokens the agent reported,
    /// `None` when it reports none (EA§6: never estimated); an `Err` is
    /// shown as the turn's non-fatal `Error`.
    async fn turn(
        &self,
        turn: crate::ids::TurnId,
        prompt: String,
        events: mpsc::Sender<crate::types::Event>,
        cancel: CancellationToken,
    ) -> Result<Option<Usage>, crate::errors::CoreError>;
}

/// A plugin's `cox_model_call` (PL§7d, T33.15): the router, the budget gate
/// and the ledger, reached this way because `cox-plugin` may not depend on
/// `cox-core` (AGENTS.md's trust-boundary rule — anything that crosses a
/// crate boundary lives behind a trait defined here). The session installs
/// its own implementation into `HostEnv` (`cox-plugin::hostfn`), which
/// blocks a plugin's worker thread on it rather than `.await`ing, since
/// that thread is not a tokio runtime worker (`cox-plugin::host`).
#[async_trait]
pub trait ModelCaller: Send + Sync {
    /// Runs `request` at `tier` — already resolved and clamped to the
    /// plugin's grant, never `think` (D5) — as job `Job::Plugin(id)`: the
    /// budget gate first (a `CoreError::Budget` refusal), then the
    /// provider call, then one ledger row, same as any other job.
    async fn call(
        &self,
        id: &str,
        tier: crate::types::Tier,
        request: crate::types::Request,
    ) -> Result<Vec<crate::types::ProviderEvent>, crate::errors::CoreError>;
}

/// A plugin's `cox_invoke_tool` (PL§4, T33.13): one tool call run the way a
/// model's is — `PreToolUse`, `Engine::decide`, the sandbox, the archive —
/// with its approval prompt naming the plugin. A trait for the same reason
/// as `ModelCaller`: `cox-plugin` may not depend on `cox-core`.
#[async_trait]
pub trait ToolInvoker: Send + Sync {
    /// Runs tool `name` with `input` for plugin `id`. A denial, a hook's
    /// block or an unknown tool is an `Ok` result with `ok: false`, as the
    /// model would see it; `Err` means the session itself failed.
    async fn invoke(
        &self,
        id: &str,
        name: &str,
        input: Value,
    ) -> Result<crate::types::ToolResult, crate::errors::CoreError>;
}

/// A decision plugin's answers to the core's typed questions (PL§4
/// "Decision points", T33.20): `cox-plugin`'s `PluginAdvisor` calls the
/// guest's `cox_decide`. Not a `Hook`, on purpose — a hook's `Modify` has no
/// monotone rule, so the core keeps the decision: it offers the options,
/// applies the point's rule and uses its static pick on silence.
#[async_trait]
pub trait Advisor: Send + Sync {
    /// The plugin id `[plugins.decide]` names this advisor by.
    fn id(&self) -> &str;

    /// Answers `question` within `budget`. Never fails: `None` is silence
    /// (not granted for the point, a trap, a timeout, garbage), and the
    /// core falls back to its static pick (D14, fail open).
    async fn advise(
        &self,
        question: crate::plugin::Question,
        budget: Duration,
    ) -> Option<crate::plugin::Advice>;
}

/// Why a push-to-talk press produced no transcript (T54.4). Each carries
/// the one line the TUI shows the user.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DictationError {
    /// Recording could not start or run: no microphone, the OS denied
    /// access, or the stream failed.
    #[error("{0}")]
    Capture(String),
    /// The model could not be loaded or the audio could not be transcribed.
    #[error("{0}")]
    Transcribe(String),
}

/// Push-to-talk dictation (P54, A123): the TUI starts and stops a recording
/// and receives text, never audio. Defined here so `cox-tui` depends on this
/// trait and never on `cox-voice`, which implements it behind `crates/cox`'s
/// `voice` feature.
#[async_trait]
pub trait Dictation: Send {
    /// Starts recording from the microphone.
    fn start(&mut self) -> Result<(), DictationError>;
    /// Stops recording and returns the transcript of what was said.
    async fn stop(&mut self) -> Result<String, DictationError>;
    /// Stops recording and discards the audio untranscribed.
    fn cancel(&mut self);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time check that the trait object types used in `ToolCx` and
    /// elsewhere are actually object-safe, since that is easy to break by
    /// accident (e.g. adding a generic method).
    #[test]
    fn archive_and_provider_are_object_safe() {
        fn assert_object_safe<T: ?Sized>() {}
        assert_object_safe::<dyn Archive>();
        assert_object_safe::<dyn Provider>();
        assert_object_safe::<dyn Tool>();
        assert_object_safe::<dyn Hook>();
        assert_object_safe::<dyn Checkpointer>();
        assert_object_safe::<dyn Relay>();
        assert_object_safe::<dyn ExternalAgent>();
        assert_object_safe::<dyn EventTap>();
        assert_object_safe::<dyn ModelCaller>();
        assert_object_safe::<dyn ToolInvoker>();
        assert_object_safe::<dyn Dictation>();
    }
}
