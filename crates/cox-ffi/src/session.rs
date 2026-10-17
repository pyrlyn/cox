// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! One open session behind the FFI (DT§4.4, §4.5): forwards to
//! `cox_app::LiveSession`, which builds it, feeds the inbox and runs each
//! intent; this side only moves the calls onto the runtime. Separate from
//! `lib.rs`, which owns what outlives one session.

use std::sync::Arc;

use cox_app::Readiness;
use cox_app::TerminalHandle as Terminal;
use cox_app::diffmodel::DiffModel;
use cox_app::live::LiveSession;
use cox_app::plugin_ui::{CanvasUpdate, PluginCanvas};
use cox_app::{
    Block, Changes, Completion, Info, Intent, LinkClick, PaletteHit, PaletteItem, PluginAction,
    PluginTab, TaskTarget, TimelinePatch, TurnCosts,
};
use cox_protocol::ids::{ArchiveId, SessionId, TaskId};
use cox_protocol::plugin::CanvasAttach;
use cox_protocol::types::TodoItem;

use crate::{AppError, on_runtime};

#[derive(uniffi::Object)]
pub struct SessionHandle {
    live: Arc<LiveSession>,
}

impl SessionHandle {
    pub(crate) fn new(live: Arc<LiveSession>) -> Arc<Self> {
        Arc::new(Self { live })
    }
}

#[uniffi::export]
impl SessionHandle {
    pub fn id(&self) -> SessionId {
        self.live.id()
    }

    /// What was skipped while the session was built (D14).
    pub fn warnings(&self) -> Vec<String> {
        self.live.warnings().to_vec()
    }

    /// Every block; the next pull continues from here.
    pub fn snapshot(&self) -> Vec<Block> {
        self.live.snapshot()
    }

    /// The next batch, at most one per frame; `None` once closed.
    pub async fn next_patches(self: Arc<Self>) -> Option<Vec<TimelinePatch>> {
        on_runtime(async move { self.live.next_patches().await })
            .await
            .ok()
            .flatten()
    }

    /// Returns at once for a turn; a fork or handoff returns its child.
    pub async fn send(
        self: Arc<Self>,
        intent: Intent,
    ) -> Result<Option<Arc<SessionHandle>>, AppError> {
        Ok(on_runtime(async move { self.live.send(intent).await })
            .await??
            .map(Self::new))
    }

    /// The provider section the code tier runs on (T60.3); `None` in an
    /// external agent's session.
    pub fn provider(&self) -> Option<String> {
        self.live.provider().map(str::to_owned)
    }

    /// Whether a turn may start now on this session's own provider (T60.3):
    /// one picked before the first turn counts, not the config's default.
    pub async fn readiness(self: Arc<Self>) -> Result<Readiness, AppError> {
        Ok(on_runtime(async move { self.live.readiness().await }).await??)
    }

    /// What the inspector's Changes tab lists (T37.29.1).
    pub async fn changes(self: Arc<Self>) -> Result<Changes, AppError> {
        Ok(on_runtime(async move { self.live.changes().await }).await??)
    }

    /// Review's diff of one changed file (T37.28.2): its checkpoint copy
    /// against the file on disk.
    pub async fn review(self: Arc<Self>, path: String) -> Result<Option<DiffModel>, AppError> {
        Ok(on_runtime(async move { self.live.review(&path).await }).await??)
    }

    /// What the inspector's Plan tab lists (T37.29.2).
    pub fn plan(&self) -> Vec<TodoItem> {
        self.live.plan()
    }

    /// What a Tasks-tab click opens (T37.29.6): a subagent's session or a
    /// finished shell's output; `None` while there is nothing to open.
    pub fn open_task(&self, task: TaskId) -> Result<Option<TaskTarget>, AppError> {
        Ok(self.live.open_task(task)?)
    }

    /// A finished shell's output, which `open_task` names (T37.22.6).
    pub fn output(&self, archive: ArchiveId) -> Result<String, AppError> {
        Ok(self.live.output(&archive)?)
    }

    /// What the inspector's Info tab lists (T37.29.5).
    pub async fn info(self: Arc<Self>) -> Result<Info, AppError> {
        Ok(on_runtime(async move { self.live.info().await }).await??)
    }

    /// The Context tab's cost history (T37.29.3.2).
    pub async fn turn_costs(self: Arc<Self>) -> Result<TurnCosts, AppError> {
        Ok(on_runtime(async move { self.live.turn_costs() }).await??)
    }

    /// `/` commands and `@` files for the composer's token.
    pub fn complete(&self, token: String, limit: u32) -> Vec<Completion> {
        self.live
            .complete(&token, usize::try_from(limit).unwrap_or(usize::MAX))
    }

    /// The command palette's rows for `query`: `items` plus this session's
    /// `/` commands and `@` files, at most `limit` of each kind (T37.44.13).
    pub fn palette(&self, query: String, items: Vec<PaletteItem>, limit: u32) -> Vec<PaletteHit> {
        self.live
            .palette(&query, items, usize::try_from(limit).unwrap_or(usize::MAX))
    }

    /// This session's earlier prompts, newest first, for ↑ in an empty
    /// composer (T37.24.6).
    pub fn history(&self, limit: u32) -> Result<Vec<String>, AppError> {
        Ok(self
            .live
            .history(usize::try_from(limit).unwrap_or(usize::MAX))?)
    }

    /// A terminal pane in this session's cwd, under its sandbox (T51.3).
    pub fn open_terminal(&self, cols: u16, rows: u16) -> Result<Arc<TerminalHandle>, AppError> {
        Ok(TerminalHandle::new(self.live.open_terminal(cols, rows)?))
    }

    /// The window's area in cells, for plugin panels and overlays.
    pub fn plugin_area(&self, width: u16, height: u16) {
        self.live.plugin_area(width, height);
    }

    /// The plugins' inspector tabs (PL§15.4), for the strip after Info.
    pub fn plugin_tabs(&self) -> Vec<PluginTab> {
        self.live.plugin_tabs()
    }

    /// The plugins' toolbar items (PL§15.4), after their status segments.
    pub fn plugin_toolbar(&self) -> Vec<PluginAction> {
        self.live.plugin_toolbar()
    }

    /// The inspector shows `plugin`'s tab, or a built-in one (`None`),
    /// `width`×`height` cells large; only the selected tab renders.
    pub fn select_plugin_tab(&self, plugin: Option<String>, width: u16, height: u16) {
        self.live
            .select_plugin_tab(plugin.as_deref(), width, height);
    }

    /// A toolbar item, palette action or notification button
    /// (`plugin:<id>:<command>`); false for an id no plugin declared.
    pub fn plugin_action(&self, id: String) -> bool {
        self.live.plugin_action(&id)
    }

    /// A click on a link in a plugin's output (PL§15.4).
    pub fn open_link(&self, link: String) -> LinkClick {
        self.live.open_link(&link)
    }

    /// Esc on a plugin overlay.
    pub fn close_plugin_overlay(&self) {
        self.live.close_plugin_overlay();
    }

    /// The canvases plugins declared (plugin-canvas §3), for the pane.
    pub fn plugin_canvases(&self) -> Vec<PluginCanvas> {
        self.live.plugin_canvases()
    }

    /// The pane shows `plugin`'s canvas at this size, newly or resized;
    /// what it draws arrives through `next_canvas`.
    pub async fn show_canvas(
        self: Arc<Self>,
        plugin: String,
        width_pt: u32,
        height_pt: u32,
    ) -> Result<(), AppError> {
        on_runtime(async move { self.live.show_canvas(&plugin, width_pt, height_pt) }).await
    }

    /// The pane hid `plugin`'s canvas.
    pub fn hide_canvas(&self, plugin: String) {
        self.live.hide_canvas(&plugin);
    }

    /// The next canvas update; `None` once the session is gone.
    pub async fn next_canvas(self: Arc<Self>) -> Option<CanvasUpdate> {
        on_runtime(async move { self.live.next_canvas().await })
            .await
            .ok()
            .flatten()
    }

    /// A click on a frame (§5): the composer's attachment, if any.
    pub async fn select_canvas(
        self: Arc<Self>,
        plugin: String,
        frame: String,
        value: String,
    ) -> Option<CanvasAttach> {
        on_runtime(async move { self.live.select_canvas(&plugin, &frame, &value).await })
            .await
            .ok()
            .flatten()
    }

    /// Stops the pull; the session keeps running (DT§4.5).
    pub fn close(&self) {
        self.live.close();
    }

    /// Quitting: ends every turn and kills what it detached (T38.2).
    pub fn end(&self) {
        self.live.end();
    }
}

/// A terminal pane's shell (T51.4): the user's own terminal, so bytes in
/// and bytes out, which SwiftTerm interprets; nothing here reaches the
/// session's events. Dropping the last reference closes it.
#[derive(uniffi::Object)]
pub struct TerminalHandle {
    term: Terminal,
}

impl TerminalHandle {
    pub(crate) fn new(term: Terminal) -> Arc<Self> {
        Arc::new(Self { term })
    }
}

#[uniffi::export]
impl TerminalHandle {
    /// Keys and pastes, as the terminal view encodes them.
    pub fn write(&self, bytes: Vec<u8>) -> Result<(), AppError> {
        Ok(self.term.write(&bytes)?)
    }

    /// The pane's new size in cells.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), AppError> {
        Ok(self.term.resize(cols, rows)?)
    }

    /// The next bytes the shell wrote; `None` once it exited and drained.
    pub async fn next_output(self: Arc<Self>) -> Option<Vec<u8>> {
        on_runtime(async move { self.term.next_output().await })
            .await
            .ok()
            .flatten()
    }

    /// The shell's exit code, once it exited.
    pub fn exit_status(&self) -> Option<u32> {
        self.term.exit_status()
    }

    /// A job other than the shell holds the foreground, so closing asks first.
    pub fn is_busy(&self) -> bool {
        self.term.is_busy()
    }

    /// Hangs up the shell and kills its process groups.
    pub fn close(&self) {
        self.term.close();
    }
}
