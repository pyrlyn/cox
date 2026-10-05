// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox-app` (DT§4.3): the UI-agnostic application core the desktop app
//! drives through `cox-ffi`: the pure fold over the core's `Event` stream,
//! the drain task that feeds it (`controller`) and the sessions themselves
//! (`app`, `live`), kept out of every surface crate so the TUI could share
//! it later and no UI toolkit leaks in (`crates/cox/tests/deps.rs`).

// why: this crate was not under `missing_docs` before the workspace lints;
// drop this allow once its public items are documented.
#![allow(missing_docs)]

pub mod app;
pub mod best_of;
pub mod browser;
pub mod changes;
pub mod coalesce;
pub mod complete;
pub mod controller;
pub mod costs;
pub mod external;
pub mod inbox;
pub mod info;
pub mod intent;
pub mod live;
pub mod mcp_login;
pub mod mcp_status;
pub mod meter_text;
pub mod models;
pub mod onboarding;
pub mod palette;
pub mod patch;
pub mod permissions;
pub mod plugin_ui;
pub mod remote;
pub mod review;
pub mod server;
pub mod settings;
pub mod settings_fields;
pub mod status;
pub mod summary;
pub mod tasks;
pub mod terminal;
pub mod timeline;
pub mod usage;
pub mod welcome;
pub mod wire;
pub mod workspace;

pub use best_of::{
    BestOf, BestOfError, BestOfId, BestOfRequest, Candidate, CandidateState, CandidateView, Launch,
    Launched, Picked,
};
pub use browser::{Browser, BrowserError, PageText};
pub use changes::{ChangedFile, Changes, Checkpoint, FileChange, TurnFiles};
pub use complete::{Completer, Completion};
pub use controller::Controller;
pub use costs::{BudgetRow, CostRow, DaySummary, TurnCosts};
pub use external::AgentChoice;
pub use inbox::{Activity, Inbox, InboxItem, InboxStatus, Need};
pub use info::{ConfigSource, Fact, Info};
pub use intent::{AgentDispatch, Dispatch, Intent, IntentError, agent_dispatch, dispatch};
pub use mcp_login::{LoginAction, McpLogin, McpServer};
pub use mcp_status::McpStatus;
pub use meter_text::{ContextPart, MeterRow, MeterText};
pub use models::{MenuModel, ModelChoice, ModelSection};
pub use onboarding::{CheckId, CheckRow, CheckStatus};
pub use palette::{PaletteHit, PaletteItem, PaletteKind};
pub use patch::{Block, BlockId, BlockKind, Status, TaskState, TimelinePatch, ToolState};
pub use permissions::{PermissionRule, RuleKind, SessionGrant};
pub use plugin_ui::{KeyValueRow, PluginKey, PluginSlot, SpanView, WidgetView};
pub use settings::{Dropped, Layer, Setting, SettingKind, SettingsError, SettingsView};
pub use settings_fields::{KeyError, SettingControl, SettingInput, SettingOption, SettingsGroup};
pub use summary::Icon;
pub use tasks::{TaskKind, TaskTarget};
pub use terminal::{TerminalError, TerminalHandle};
pub use timeline::Timeline;
pub use usage::{Meter, Tally, TurnUsage, UsageView};
pub use welcome::{Suggestion, Welcome};
pub use workspace::{Project, SearchHit, SessionEntry, Workspace, WorkspaceError};

// What the exported types carry, named here so `cox-ffi` depends on no
// other workspace crate (T37.39, DT§4.2).
pub use cox_render::diffmodel;
pub use cox_render::doc;
pub use cox_store::fts::SessionInfo;
pub use cox_store::lock::Holder;
pub use cox_tools::git::Linked;
