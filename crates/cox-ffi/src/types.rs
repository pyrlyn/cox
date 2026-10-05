// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The records and enums Swift sees (DT§4.4): cox-app's and cox-protocol's
//! own types, declared to UniFFI with `#[uniffi::remote]` rather than
//! mirrored, so there is no conversion code and a field added upstream
//! fails this build instead of drifting. Separate from the exported objects
//! because these are data only. Ids and paths cross as strings; the one
//! type UniFFI cannot carry as-is (a span's `[u8; 3]` colour) crosses as
//! the local `Span`, and the records only this surface has (`OpenRequest`,
//! and `BestOfLaunch`, which carries session handles) are declared here too.

use std::path::PathBuf;
use std::sync::Arc;

use cox_app::AgentChoice;
use cox_app::Holder;
use cox_app::SessionInfo;
use cox_app::best_of::{
    BestOf, BestOfId, BestOfRequest, Candidate, CandidateState, CandidateView, Launch, Launched,
    Picked,
};
use cox_app::complete::{Splice, TypedToken};
use cox_app::diffmodel::{DiffHunk, DiffLine, DiffLineKind, DiffModel, WordRange};
use cox_app::doc::{Block as DocBlock, StyledDoc, StyledSpan, TextKind, TextLine};
use cox_app::intent::{DraftIntent, DraftKind, SendWhen};
use cox_app::onboarding::{CheckId, CheckRow, CheckStatus};
use cox_app::patch::TaskState;
use cox_app::patch::{Block, BlockId, BlockKind, Status, TimelinePatch, ToolState};
use cox_app::review::LineComment;
use cox_app::workspace::{SidebarKind, SidebarRow, SidebarSection, SidebarStatus, SubtitlePart};
use cox_app::{
    Activity, BrowserError, BudgetRow, ChangedFile, Changes, Checkpoint, Completion, ConfigSource,
    ContextPart, CostRow, DaySummary, Dropped, FileChange, Icon, InboxItem, InboxStatus, Info,
    Intent, Layer, Linked, LoginAction, McpLogin, McpServer, McpStatus, MeterRow, MeterText,
    ModelChoice, Need, PageText, PaletteHit, PaletteItem, PaletteKind, Project, SearchHit,
    SessionEntry, Setting, SettingKind, SettingsView, Tally, TaskKind, TaskTarget, TurnCosts,
    TurnUsage, UsageView,
};
use cox_app::{Fact, TurnFiles};
use cox_app::{KeyError, PermissionRule, RuleKind, SessionGrant, SettingsGroup};
use cox_app::{KeyValueRow, PluginKey, PluginSlot, SpanView, WidgetView};
use cox_app::{MenuModel, ModelSection};
use cox_app::{SettingControl, SettingInput, SettingOption};
use cox_app::{Suggestion, Welcome};
use cox_protocol::ids::{ArchiveId, CallId, SessionId, TaskId, TurnId};
use cox_protocol::plugin::Slot;
use cox_protocol::plugin::ui::StyleToken;
use cox_protocol::traits::{FileStat, Worktree, WorktreeInfo};
use cox_protocol::types::{
    ApprovalPolicy, ArchiveRef, Attachment, CompactReason, DecidedBy, Decision, Effort, Level,
    ModelId, PermissionMode, Risk, Segments, Source, StopReason, Tier, TodoItem, TodoState,
    ToolCall, Usage, Why,
};
use serde_json::Value;

use crate::session::SessionHandle;

macro_rules! string_ids {
    ($($id:ident),*) => {$(
        uniffi::custom_type!($id, String, {
            remote,
            lower: |id| id.to_string(),
            try_lift: |s| Ok(s.parse()?),
        });
    )*};
}
string_ids!(SessionId, TurnId, CallId, ArchiveId, TaskId);

uniffi::custom_type!(BlockId, String, {
    remote,
    lower: |id| id.0,
    try_lift: |s| Ok(BlockId(s)),
});
uniffi::custom_type!(ModelId, String, {
    remote,
    lower: |id| id.0,
    try_lift: |s| Ok(ModelId(s)),
});
uniffi::custom_type!(PathBuf, String, {
    remote,
    lower: |path| path.to_string_lossy().into_owned(),
    try_lift: |s| Ok(PathBuf::from(s)),
});
// A tool's input and an edited approval: JSON text, as on the wire.
uniffi::custom_type!(Value, String, {
    remote,
    lower: |value| value.to_string(),
    try_lift: |s| Ok(serde_json::from_str(&s)?),
});
uniffi::custom_type!(StyledSpan, Span, {
    remote,
    lower: |s| Span {
        rgb: s.rgb.map(|[r, g, b]| u32::from_be_bytes([0, r, g, b])),
        light: s.light.map(|[r, g, b]| u32::from_be_bytes([0, r, g, b])),
        text: s.text,
        token: s.token,
        bold: s.bold,
        italic: s.italic,
        strike: s.strike,
        underline: s.underline,
        link: s.link,
    },
    try_lift: |s| Ok(StyledSpan {
        rgb: s.rgb.map(|c| {
            let [_, r, g, b] = c.to_be_bytes();
            [r, g, b]
        }),
        light: s.light.map(|c| {
            let [_, r, g, b] = c.to_be_bytes();
            [r, g, b]
        }),
        text: s.text,
        token: s.token,
        bold: s.bold,
        italic: s.italic,
        strike: s.strike,
        underline: s.underline,
        link: s.link,
    }),
});

#[uniffi::remote(Record)]
pub struct ModelChoice {
    pub tier: Tier,
    pub provider: String,
    pub id: String,
    pub display_name: Option<String>,
    pub short_name: Option<String>,
    pub efforts: Vec<Effort>,
    pub context_window: Option<u32>,
}

#[uniffi::remote(Record)]
pub struct Welcome {
    pub summary: String,
    pub suggestions: Vec<Suggestion>,
}

#[uniffi::remote(Record)]
pub struct Suggestion {
    pub title: String,
    pub detail: String,
    pub prompt: String,
}

#[uniffi::remote(Record)]
pub struct ModelSection {
    pub tier: Tier,
    pub title: String,
    pub models: Vec<MenuModel>,
}

#[uniffi::remote(Record)]
pub struct MenuModel {
    pub id: String,
    pub display_name: Option<String>,
    pub short_name: Option<String>,
    pub efforts: String,
}

#[uniffi::remote(Record)]
pub struct Project {
    pub root: PathBuf,
    pub name: String,
    pub sessions: u64,
    pub cost_usd: f64,
    pub updated_at: String,
}

/// What `App::open` opens: a new session in `cwd`, or `resume`'s.
/// `theme` is the syntect theme code blocks are highlighted with; a diff
/// takes its dark and light variant (A95). `agent` names the external ACP
/// agent a new session is driven by, `None` for cox (T52.7).
#[derive(uniffi::Record)]
pub struct OpenRequest {
    pub cwd: String,
    pub resume: Option<SessionId>,
    pub theme: String,
    pub agent: Option<String>,
}

#[uniffi::remote(Record)]
pub struct AgentChoice {
    pub name: Option<String>,
    pub label: String,
    pub origin: String,
    pub launch: String,
    pub unavailable: Option<String>,
}

/// `StyledSpan` with its theme colours as `0xRRGGBB`.
#[derive(uniffi::Record)]
pub struct Span {
    pub text: String,
    pub token: StyleToken,
    pub rgb: Option<u32>,
    pub light: Option<u32>,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub underline: bool,
    pub link: Option<String>,
}

#[uniffi::remote(Enum)]
pub enum TimelinePatch {
    Reset {
        blocks: Vec<Block>,
    },
    Upsert {
        block: Box<Block>,
        after: Option<BlockId>,
    },
    AppendText {
        id: BlockId,
        text: String,
    },
    DocTail {
        id: BlockId,
        from: u32,
        blocks: Vec<DocBlock>,
    },
    Remove {
        id: BlockId,
    },
    Usage {
        usage: Box<UsageView>,
    },
    Status {
        status: Status,
    },
    PluginSlot {
        slot: Box<PluginSlot>,
    },
}

/// One plugin slot (T52.14, PL§8); the app draws `view` natively.
#[uniffi::remote(Record)]
pub struct PluginSlot {
    pub plugin: String,
    pub slot: Slot,
    pub view: Option<WidgetView>,
    pub visible: bool,
    pub stopped: bool,
}

#[uniffi::remote(Enum)]
pub enum Slot {
    StatusLeft,
    StatusRight,
    Panel,
    Overlay,
}

/// PL§8's closed widget tree, sanitized and bounded by cox-app; it recurses
/// only through lists, which every binding carries as plain arrays.
#[uniffi::remote(Enum)]
pub enum WidgetView {
    Text {
        lines: Vec<Vec<SpanView>>,
    },
    List {
        items: Vec<Vec<SpanView>>,
        selected: Option<u32>,
    },
    Table {
        header: Vec<SpanView>,
        rows: Vec<Vec<SpanView>>,
        widths: Vec<u16>,
    },
    KeyValue {
        rows: Vec<KeyValueRow>,
    },
    Gauge {
        ratio: f64,
        label: SpanView,
    },
    Stack {
        vertical: bool,
        children: Vec<WidgetView>,
        sizes: Vec<u16>,
    },
    Block {
        title: Option<SpanView>,
        child: Vec<WidgetView>,
    },
}

#[uniffi::remote(Record)]
pub struct SpanView {
    pub text: String,
    pub style: StyleToken,
    pub bold: bool,
    pub italic: bool,
}

#[uniffi::remote(Record)]
pub struct KeyValueRow {
    pub key: SpanView,
    pub value: Vec<SpanView>,
}

#[uniffi::remote(Record)]
pub struct PluginKey {
    pub plugin: String,
    pub key: String,
    pub name: String,
    pub description: String,
}

#[uniffi::remote(Record)]
pub struct Status {
    pub queued: u32,
    pub mode: Option<PermissionMode>,
    pub next_mode: Option<PermissionMode>,
    pub model: Option<ModelId>,
    pub model_name: Option<String>,
    pub short_name: Option<String>,
    pub effort: Option<Effort>,
}

#[uniffi::remote(Record)]
pub struct Block {
    pub id: BlockId,
    pub turn: u32,
    pub kind: BlockKind,
}

#[uniffi::remote(Enum)]
pub enum BlockKind {
    User {
        text: String,
        attachments: Vec<String>,
    },
    Assistant {
        text: String,
        doc: StyledDoc,
        plugin_view: Option<WidgetView>,
    },
    Thinking {
        text: String,
        duration_ms: Option<u64>,
    },
    Tool {
        tool: String,
        summary: String,
        icon: Icon,
        risk: Risk,
        state: ToolState,
        tail: String,
        archive: Option<ArchiveRef>,
        diff: Option<DiffModel>,
        duration_ms: u64,
        plugin_view: Option<WidgetView>,
    },
    ToolGroup {
        summary: String,
        children: Vec<BlockId>,
        state: ToolState,
    },
    Approval {
        call: CallId,
        tool: String,
        summary: String,
        input: Value,
        grants: Vec<String>,
        why: Why,
        source: Option<Source>,
        decision: Option<Decision>,
        by: Option<DecidedBy>,
    },
    Question {
        call: CallId,
        question: String,
        options: Vec<String>,
        answer: Option<String>,
    },
    Task {
        task: TaskId,
        label: String,
        tier: Tier,
        done: bool,
        cost_usd: f64,
        exit_code: Option<i32>,
        state: TaskState,
        kind: TaskKind,
    },
    Compaction {
        before_tokens: u32,
        after_tokens: u32,
        reason: CompactReason,
        summary: Option<String>,
    },
    Checkpoint {
        files: Vec<PathBuf>,
    },
    Notice {
        level: Level,
        text: String,
    },
    Error {
        text: String,
        fatal: bool,
    },
    TurnMeta {
        model: ModelId,
        tier: Tier,
        usage: Option<Usage>,
        stop: Option<StopReason>,
    },
}

#[uniffi::remote(Enum)]
pub enum ToolState {
    Running,
    Done,
    Failed,
}

#[uniffi::remote(Enum)]
pub enum TaskState {
    Running,
    Succeeded,
    Failed,
}

#[uniffi::remote(Enum)]
pub enum TaskKind {
    Agent,
    Shell,
}

#[uniffi::remote(Enum)]
pub enum TaskTarget {
    Transcript { session: SessionId },
    Output { archive: ArchiveId },
}

#[uniffi::remote(Enum)]
pub enum Icon {
    Read,
    Edit,
    Shell,
    Search,
    Web,
    Todo,
    Ask,
    Agent,
    Mcp,
    Tool,
}

#[uniffi::remote(Record)]
pub struct StyledDoc {
    pub blocks: Vec<DocBlock>,
}

#[uniffi::remote(Enum)]
pub enum DocBlock {
    Text {
        kind: TextKind,
        lines: Vec<TextLine>,
    },
    Code {
        lang: String,
        lines: Vec<Vec<StyledSpan>>,
    },
    Table {
        rows: Vec<Vec<String>>,
    },
    Rule,
}

#[uniffi::remote(Record)]
pub struct TextLine {
    pub quote: u8,
    pub depth: u8,
    pub marker: String,
    pub spans: Vec<StyledSpan>,
}

#[uniffi::remote(Enum)]
pub enum TextKind {
    Paragraph,
    Heading(u8),
    List,
    Quote,
}

#[uniffi::remote(Enum)]
pub enum StyleToken {
    Text,
    Dim,
    Accent,
    User,
    Agent,
    Tool,
    Ok,
    Warn,
    Error,
    DiffAdd,
    DiffDel,
    DiffHunk,
    Border,
    Selection,
}

#[uniffi::remote(Record)]
pub struct UsageView {
    pub session: Tally,
    pub turn: Option<TurnUsage>,
    pub context_tokens: u32,
    pub text: MeterText,
}

#[uniffi::remote(Record)]
pub struct MeterText {
    pub sent: String,
    pub received: String,
    pub rate: String,
    pub spoken: String,
    pub heading: String,
    pub phase: String,
    pub rate_unit: String,
    pub rate_detail: String,
    pub rows: Vec<MeterRow>,
    pub context: String,
    pub context_share: String,
    pub context_parts: Vec<ContextPart>,
    pub context_free: String,
    pub cache_hit: String,
    pub cache_hit_session: String,
    pub footnote: String,
    pub context_percent: String,
    pub context_fill: f64,
    pub cost: String,
}

#[uniffi::remote(Record)]
pub struct ContextPart {
    pub kind: String,
    pub label: String,
    pub tokens: String,
    pub share: f64,
}

#[uniffi::remote(Record)]
pub struct MeterRow {
    pub label: String,
    pub turn: String,
    pub session: String,
    pub detail: bool,
}

#[uniffi::remote(Record)]
pub struct Tally {
    pub sent: u32,
    pub received: u32,
    pub cache_read: u32,
    pub cache_write: u32,
    pub uncached: u32,
    pub cost_usd: f64,
    pub calls: u32,
    pub estimated: bool,
}

#[uniffi::remote(Record)]
pub struct TurnUsage {
    pub turn: TurnId,
    pub tally: Tally,
    pub thinking_tokens: u32,
    pub ttft_ms: Option<u64>,
    pub tok_per_s: Option<f64>,
    pub exact: bool,
    pub sparkline: Vec<f64>,
    pub done: bool,
}

#[uniffi::remote(Enum)]
pub enum Intent {
    Send {
        text: String,
        attachments: Vec<Attachment>,
        confirm_think: bool,
    },
    Approve {
        call: CallId,
        decision: Decision,
    },
    Answer {
        question: CallId,
        text: Option<String>,
    },
    Interrupt,
    Queue {
        text: String,
        attachments: Vec<Attachment>,
        confirm_think: bool,
    },
    Compact {
        focus: Option<String>,
    },
    SetMode {
        mode: PermissionMode,
    },
    SwitchModel {
        tier: Tier,
        model: Option<ModelId>,
    },
    SetEffort {
        effort: Option<Effort>,
    },
    Rewind {
        to_turn: u32,
        code: bool,
        conversation: bool,
    },
    Redo,
    RevertFile {
        path: String,
        to_turn: u32,
    },
    RevertHunk {
        path: String,
        to_turn: u32,
        hunk: u32,
        now_digest: String,
    },
    Fork {
        turn: Option<u32>,
    },
    Handoff {
        objective: String,
    },
    Background {
        call: CallId,
    },
    Shell {
        command: String,
        share: bool,
    },
    Command {
        line: String,
    },
    Rename {
        title: String,
    },
}

#[uniffi::remote(Record)]
pub struct InboxItem {
    pub session: SessionId,
    pub source: Option<Source>,
    pub need: Need,
    pub expired: bool,
    pub seq: u64,
    pub title: String,
    pub subtitle: String,
    pub status: InboxStatus,
}

#[uniffi::remote(Enum)]
pub enum InboxStatus {
    Waiting,
    Idle,
    Error,
}

#[uniffi::remote(Enum)]
pub enum Need {
    Approval {
        call: ToolCall,
        why: Why,
    },
    Question {
        call_id: CallId,
        question: String,
        options: Vec<String>,
    },
    Failed {
        text: String,
    },
    TaskDone {
        task: TaskId,
        label: String,
        ok: bool,
    },
}

#[uniffi::remote(Enum)]
pub enum Activity {
    Idle,
    Running,
    WaitingOnYou,
    Failed,
}

#[uniffi::remote(Record)]
pub struct SessionEntry {
    pub info: SessionInfo,
    pub held_by: Option<Holder>,
    pub agent: Option<String>,
    pub best_of: Option<String>,
}

#[uniffi::remote(Enum)]
pub enum SidebarStatus {
    Running,
    Waiting,
    Idle,
    Error,
}

#[uniffi::remote(Enum)]
pub enum SubtitlePart {
    Text { text: String },
    Age { updated_at: String },
}

#[uniffi::remote(Enum)]
pub enum SidebarKind {
    Section { count: Option<String> },
    Project { is_expanded: bool },
}

#[uniffi::remote(Record)]
pub struct SidebarRow {
    pub id: String,
    pub session: SessionId,
    pub status: SidebarStatus,
    pub title: String,
    pub subtitle: Vec<SubtitlePart>,
    pub cost: Option<f64>,
    pub is_read_only: bool,
}

#[uniffi::remote(Record)]
pub struct SidebarSection {
    pub id: String,
    pub title: String,
    pub kind: SidebarKind,
    pub rows: Vec<SidebarRow>,
}

#[uniffi::remote(Record)]
pub struct SessionInfo {
    pub id: String,
    pub title: Option<String>,
    pub cwd: String,
    pub created_at: String,
    pub updated_at: String,
    pub turns: i64,
    pub cost_usd: f64,
}

#[uniffi::remote(Record)]
pub struct Holder {
    pub pid: u32,
    pub surface: String,
    pub since: String,
}

#[uniffi::remote(Record)]
pub struct SearchHit {
    pub session: SessionInfo,
    pub turn: i64,
    pub snippet: String,
}

#[uniffi::remote(Record)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub main: bool,
    pub locked: Option<String>,
    pub stale: bool,
    pub merged: bool,
    pub bytes: u64,
}

#[uniffi::remote(Record)]
pub struct LineComment {
    pub path: String,
    pub line: u32,
    pub removed: bool,
    pub text: String,
}

#[uniffi::remote(Record)]
pub struct Changes {
    pub files: Vec<ChangedFile>,
    pub checkpoints: Vec<Checkpoint>,
    pub worktree: Option<Linked>,
    pub worktree_facts: Vec<Fact>,
    pub turns: Vec<TurnFiles>,
}

#[uniffi::remote(Record)]
pub struct TurnFiles {
    pub turn: u32,
    pub files: Vec<ChangedFile>,
}

#[uniffi::remote(Record)]
pub struct ChangedFile {
    pub path: PathBuf,
    pub change: FileChange,
    pub added: u32,
    pub removed: u32,
    pub call: CallId,
    pub turn: u32,
}

#[uniffi::remote(Record)]
pub struct TodoItem {
    pub id: String,
    pub text: String,
    pub state: TodoState,
}

#[uniffi::remote(Enum)]
pub enum TodoState {
    Pending,
    InProgress,
    Done,
}

#[uniffi::remote(Enum)]
pub enum FileChange {
    Edited,
    Created,
    Deleted,
}

#[uniffi::remote(Record)]
pub struct Checkpoint {
    pub turn: u32,
    pub label: String,
    pub time: String,
}

#[uniffi::remote(Record)]
pub struct Info {
    pub session: SessionId,
    pub cwd: PathBuf,
    pub worktree: Option<Linked>,
    pub config: Vec<ConfigSource>,
    pub rollout: PathBuf,
    pub facts: Vec<Fact>,
    pub config_facts: Vec<Fact>,
}

#[uniffi::remote(Record)]
pub struct Fact {
    pub label: String,
    pub value: Option<String>,
    pub detail: bool,
}

#[uniffi::remote(Record)]
pub struct TurnCosts {
    pub columns: Vec<String>,
    pub rows: Vec<CostRow>,
    pub total: CostRow,
    pub project: String,
    pub budget: Vec<BudgetRow>,
}

#[uniffi::remote(Record)]
pub struct BudgetRow {
    pub label: String,
    pub text: String,
    pub fraction: Option<f64>,
}

#[uniffi::remote(Record)]
pub struct CostRow {
    pub label: String,
    pub values: Vec<String>,
    pub detail: bool,
}

#[uniffi::remote(Record)]
pub struct ConfigSource {
    pub layer: Layer,
    pub file: Option<PathBuf>,
    pub keys: u32,
}

#[uniffi::remote(Record)]
pub struct Linked {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub base: Option<String>,
    pub commit: Option<String>,
    pub bytes: u64,
}

#[uniffi::remote(Record)]
pub struct SettingsView {
    pub settings: Vec<Setting>,
    pub user_file: PathBuf,
    pub project_file: Option<PathBuf>,
    pub mcp: Vec<McpServer>,
    pub dropped: Vec<Dropped>,
    pub rules: Vec<PermissionRule>,
    pub grants: Vec<SessionGrant>,
    pub providers: Vec<String>,
}

#[uniffi::remote(Record)]
pub struct PermissionRule {
    pub kind: RuleKind,
    pub rule: String,
    pub layer: Layer,
    pub editable: bool,
}

#[uniffi::remote(Enum)]
pub enum RuleKind {
    Allow,
    Ask,
    Deny,
}

#[uniffi::remote(Record)]
pub struct SessionGrant {
    pub session: SessionId,
    pub title: Option<String>,
    pub tool: String,
    pub subject: String,
}

#[uniffi::remote(Record)]
pub struct CheckRow {
    pub id: CheckId,
    pub status: CheckStatus,
    pub detail: String,
}

#[uniffi::remote(Enum)]
pub enum CheckId {
    ProviderKey,
    Git,
    Sandbox,
    ShellEnv,
}

#[uniffi::remote(Enum)]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

#[uniffi::remote(Record)]
pub struct Dropped {
    pub key: String,
    pub value: String,
    pub kept: String,
    pub reason: String,
    pub group: SettingsGroup,
    pub change: String,
}

#[uniffi::remote(Record)]
pub struct McpServer {
    pub name: String,
    pub source: String,
    pub login: McpLogin,
    pub status: McpStatus,
    pub log: Vec<String>,
    pub detail: String,
    pub action: Option<LoginAction>,
}

#[uniffi::remote(Enum)]
pub enum LoginAction {
    LogIn,
    LogOut,
}

#[uniffi::remote(Enum)]
pub enum McpStatus {
    Connected,
    NeedsLogin,
    Failed,
    Disabled,
    Unknown,
}

#[uniffi::remote(Enum)]
pub enum McpLogin {
    Stdio,
    LoggedOut,
    LoggedIn { expires: Option<String> },
    Expired,
    Unreadable { error: String },
}

#[uniffi::remote(Record)]
pub struct Setting {
    pub key: String,
    pub value: Value,
    pub layer: Layer,
    pub editable: bool,
    pub kind: SettingKind,
    pub description: String,
    pub group: SettingsGroup,
    pub title: String,
    pub table: Option<String>,
    pub provider: Option<String>,
    pub detail: Option<String>,
    pub control: SettingControl,
}

#[uniffi::remote(Enum)]
pub enum SettingControl {
    Toggle {
        on: bool,
    },
    Slider {
        value: f64,
        min: f64,
        max: f64,
        text: String,
    },
    Choice {
        value: String,
        options: Vec<String>,
    },
    Menu {
        value: String,
        options: Vec<SettingOption>,
    },
    Field {
        text: String,
    },
    Json {
        text: String,
    },
}

#[uniffi::remote(Record)]
pub struct SettingOption {
    pub value: String,
    pub title: String,
}

#[uniffi::remote(Enum)]
pub enum SettingInput {
    Bool { value: bool },
    Integer { value: i64 },
    Number { value: f64 },
    Text { value: String },
    List { values: Vec<String> },
}

#[uniffi::remote(Enum)]
pub enum SettingsGroup {
    General,
    Models,
    Permissions,
    Sandbox,
    Budget,
    Mcp,
    Plugins,
    Appearance,
    Advanced,
}

#[uniffi::remote(Error)]
pub enum KeyError {
    Empty,
    UnknownProvider { provider: String },
}

#[uniffi::remote(Enum)]
pub enum Layer {
    Default,
    User,
    Project,
    ClaudeSettings,
    Env,
    Flag,
}

#[uniffi::remote(Enum)]
pub enum SettingKind {
    Toggle,
    Integer { min: Option<f64>, max: Option<f64> },
    Number { min: Option<f64>, max: Option<f64> },
    Text,
    Choice { options: Vec<String> },
    List,
    Other,
}

#[uniffi::remote(Record)]
pub struct Completion {
    pub insert: String,
    pub detail: String,
}

#[uniffi::remote(Record)]
pub struct TypedToken {
    pub start: u32,
    pub end: u32,
    pub text: String,
}

#[uniffi::remote(Record)]
pub struct Splice {
    pub text: String,
    pub caret: u32,
}

#[uniffi::remote(Enum)]
pub enum SendWhen {
    Queue,
    Now,
}

#[uniffi::remote(Enum)]
pub enum DraftKind {
    Shell,
    Command,
    Turn,
}

#[uniffi::remote(Record)]
pub struct DraftIntent {
    pub kind: DraftKind,
    pub queued: bool,
    pub can_send: bool,
    pub keeps_attachments: bool,
    pub enters_shell: bool,
}

#[uniffi::remote(Enum)]
pub enum PaletteKind {
    Action,
    Session,
    Command,
    File,
}

#[uniffi::remote(Record)]
pub struct PaletteItem {
    pub kind: PaletteKind,
    pub id: String,
    pub title: String,
    pub detail: String,
}

#[uniffi::remote(Record)]
pub struct PaletteHit {
    pub item: PaletteItem,
    pub matched: Vec<u32>,
}

#[uniffi::remote(Record)]
pub struct ToolCall {
    pub id: CallId,
    pub name: String,
    pub input: Value,
    pub risk: Risk,
    pub subject: String,
    pub segments: Option<Segments>,
}

#[uniffi::remote(Record)]
pub struct Segments {
    pub commands: Vec<String>,
    pub opaque: bool,
}

#[uniffi::remote(Record)]
pub struct Attachment {
    pub name: String,
    pub media_type: String,
    pub data_b64: String,
}

#[uniffi::remote(Record)]
pub struct DiffModel {
    pub path: PathBuf,
    pub hunks: Vec<DiffHunk>,
    pub digest: Option<String>,
}

#[uniffi::remote(Record)]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
    pub index: u32,
}

#[uniffi::remote(Record)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub spans: Vec<StyledSpan>,
    pub words: Vec<WordRange>,
}

#[uniffi::remote(Record)]
pub struct WordRange {
    pub start: u32,
    pub end: u32,
}

#[uniffi::remote(Enum)]
pub enum DiffLineKind {
    Context,
    Add,
    Del,
}

#[uniffi::remote(Record)]
pub struct ArchiveRef {
    pub id: ArchiveId,
    pub bytes: u64,
}

#[uniffi::remote(Record)]
pub struct Source {
    pub session: SessionId,
    pub agent: Option<String>,
    pub preset: Option<String>,
}

#[uniffi::remote(Record)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_write_tokens: u32,
    pub estimated: bool,
    pub cost_usd: f64,
    pub latency_ms: u64,
}

#[uniffi::remote(Enum)]
pub enum Decision {
    Allow,
    AllowForSession,
    Deny { reason: String },
    Edit { input: Value },
}

#[uniffi::remote(Enum)]
pub enum Why {
    RuleAsk { rule: String },
    Risk { risk: Risk },
    SandboxDenied { detail: String },
    Policy { policy: ApprovalPolicy },
}

#[uniffi::remote(Enum)]
pub enum StopReason {
    EndTurn,
    MaxTurns,
    Interrupted,
    Budget,
    Refusal { detail: String },
    Error,
}

#[uniffi::remote(Enum)]
pub enum ApprovalPolicy {
    Untrusted,
    OnRequest,
    OnFailure,
    Never,
}

#[uniffi::remote(Enum)]
pub enum DecidedBy {
    User,
    Rule,
    Session,
    Policy,
    Hook,
}

#[uniffi::remote(Enum)]
pub enum Risk {
    ReadOnly,
    Write,
    Exec,
    Destructive,
}

#[uniffi::remote(Enum)]
pub enum Tier {
    Cheap,
    Code,
    Think,
}

#[uniffi::remote(Enum)]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
}

#[uniffi::remote(Enum)]
pub enum PermissionMode {
    Default,
    Plan,
    Auto,
    Bypass,
}

#[uniffi::remote(Enum)]
pub enum CompactReason {
    PreCall,
    PostTurn,
    Manual,
    ContextTooLong,
}

#[uniffi::remote(Enum)]
pub enum Level {
    Info,
    Warn,
    Budget,
    Security,
}

/// T51.12: the menu bar's "Today" footer.
#[uniffi::remote(Record)]
pub struct DaySummary {
    pub cost: String,
    pub sessions: u64,
    pub text: String,
}

/// T51.8: what the app's browser pane reports of its page.
#[uniffi::remote(Record)]
pub struct PageText {
    pub title: String,
    pub url: String,
    pub text: String,
}

/// T51.8: why the Swift browser could not do what a tool asked. Local, not
/// `cox_app::BrowserError` declared remote, because a foreign trait's error
/// must also take UniFFI's unexpected-callback error, and that `From` may
/// only be written for a type of this crate.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum BrowserFailure {
    #[error("no page is open; call browser_open first")]
    NoPage,
    #[error("{message}")]
    Page { message: String },
}

impl From<uniffi::UnexpectedUniFFICallbackError> for BrowserFailure {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        Self::Page { message: e.reason }
    }
}

impl From<BrowserFailure> for BrowserError {
    fn from(e: BrowserFailure) -> Self {
        match e {
            BrowserFailure::NoPage => Self::NoPage,
            BrowserFailure::Page { message } => Self::Page(message),
        }
    }
}

// Best of n (T52.9, T52.10, T52.11): the launch, the compare view's columns
// and what a pick did.
uniffi::custom_type!(BestOfId, String, {
    remote,
    lower: |id| id.0,
    try_lift: |s| Ok(BestOfId(s)),
});

#[uniffi::remote(Enum)]
pub enum Candidate {
    Cox { model: Option<String> },
    Agent { name: String },
}

#[uniffi::remote(Record)]
pub struct BestOfRequest {
    pub project: PathBuf,
    pub prompt: String,
    pub candidates: Vec<Candidate>,
}

#[uniffi::remote(Record)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub main: PathBuf,
}

#[uniffi::remote(Record)]
pub struct Launched {
    pub candidate: Candidate,
    pub worktree: Option<Worktree>,
    pub session: Option<SessionId>,
    pub failed: Option<String>,
    pub started_ms: u64,
    pub pruned: bool,
}

#[uniffi::remote(Record)]
pub struct BestOf {
    pub id: BestOfId,
    pub project: PathBuf,
    pub prompt: String,
    pub candidates: Vec<Launched>,
    pub kept: Option<u32>,
}

#[uniffi::remote(Enum)]
pub enum CandidateState {
    Running,
    WaitingOnYou,
    Done,
    Failed { why: String },
    Kept,
    Pruned,
}

#[uniffi::remote(Record)]
pub struct FileStat {
    pub path: PathBuf,
    pub added: u32,
    pub removed: u32,
}

#[uniffi::remote(Record)]
pub struct CandidateView {
    pub candidate: Candidate,
    pub label: String,
    pub state: CandidateState,
    pub session: Option<SessionId>,
    pub worktree: Option<PathBuf>,
    pub branch: Option<String>,
    pub files: Vec<FileStat>,
    pub added: u32,
    pub removed: u32,
    pub cost_usd: f64,
    pub duration_ms: u64,
}

#[uniffi::remote(Record)]
pub struct Picked {
    pub pruned: Vec<PathBuf>,
    pub dirty: Vec<PathBuf>,
    pub refused: Vec<String>,
}

/// `cox_app::Launch` as Swift sees it: the group, and a handle on each
/// candidate session that started, in candidate order, for the windows.
/// Local because a `LiveSession` crosses only inside a `SessionHandle`.
#[derive(uniffi::Record)]
pub struct BestOfLaunch {
    pub group: BestOf,
    pub sessions: Vec<Arc<SessionHandle>>,
}

impl From<Launch> for BestOfLaunch {
    fn from(launch: Launch) -> Self {
        Self {
            group: launch.group,
            sessions: launch
                .sessions
                .into_iter()
                .map(SessionHandle::new)
                .collect(),
        }
    }
}
