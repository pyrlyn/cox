// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The desktop app's side of plugin UI (T52.14, PL§8): the closed `Widget`
//! tree a plugin renders, turned into a [`WidgetView`] whose every string went
//! through `cox_sanitize` and whose size PL§8 caps, and the per-session slot
//! state — which slots are on screen, what area each gets, when each renders
//! again and when three missed deadlines stop one — and the desktop-only
//! contributions (PL§15.4): the inspector tab, which renders only while
//! selected, and the toolbar and palette actions, each of which runs one of
//! the plugin's granted commands, and what a plugin can put in front of the
//! person (PL§15.2, §15.4): a link that opens on a click, an image from its
//! package, a notice with buttons. Separate from
//! `cox-session`'s `plugin_ui`, which only calls the hosts: this module is
//! what the app shows, as `cox-tui`'s `status.rs` is for the terminal.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use cox_protocol::ids::{ItemId, SessionId};
use cox_protocol::plugin::ui::Span;
use cox_protocol::plugin::{
    ActionDecl, CanvasAttach, CanvasDecl, CanvasError, CanvasIn, CanvasOut, CanvasSelect,
    CommandDecl, DesktopNotice, DesktopOut, Frame, MAX_IMAGE_BYTES, MAX_NOTICE_ACTIONS,
    MAX_TOOLBAR_ACTIONS, NoticeLevel, RenderIn, Slot, StyleToken, Widget, is_link, is_symbol_name,
};
use cox_protocol::types::{Event, ItemKind, ToolCall};
use cox_sanitize::sanitize;
use cox_session::plugin_ui::{ItemSource, LivePlugins, PluginAnswer, PluginHost, PluginRequest};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::Completion;
use crate::palette::{PaletteItem, PaletteKind};
use crate::patch::{BlockId, BlockKind};
use crate::timeline::key;

/// `cox_render` misses in a row that stop a slot for the session (PL§8).
pub const MAX_MISSES: u8 = 3;
/// Columns a status segment may fill (PL§8), as in the TUI.
pub const SEGMENT_COLS: u16 = 24;
/// Rows the `panel` slot may fill, above the composer (PL§8).
pub const PANEL_ROWS: u16 = 8;
/// What a tree over PL§8's limits shows instead, as in the TUI.
pub const TOO_LARGE: &str = "plugin output too large";
/// The window's area in cells until the app reports one.
const DEFAULT_AREA: (u16, u16) = (80, 24);
/// What the app draws for a symbol name outside `is_symbol_name` (PL§15.4),
/// the glyph it also falls back to for a valid name AppKit does not know.
pub const FALLBACK_SYMBOL: &str = "puzzlepiece.extension";
/// The image bytes one widget tree may carry. PL§8's node cap alone would
/// allow 512 images of `MAX_IMAGE_BYTES` each; past this the rest draw as
/// their `alt` line.
const TREE_IMAGE_BYTES: usize = 4 * MAX_IMAGE_BYTES;

/// A run of sanitized text with one colour role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanView {
    pub text: String,
    pub style: StyleToken,
    pub bold: bool,
    pub italic: bool,
    /// What a click opens, when the plugin's `Span.link` is one the app may
    /// follow (`link_target`); pass it to `open_link`.
    #[serde(default)]
    pub link: Option<String>,
}

/// One `key: value` row; a struct, not a tuple, so the FFI mirrors it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyValueRow {
    pub key: SpanView,
    pub value: Vec<SpanView>,
}

/// PL§8's `Widget`, variant for variant, as plain data a UI draws natively.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
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
    /// `ratio` clamped to `0..=1`.
    Gauge {
        ratio: f64,
        label: SpanView,
    },
    Stack {
        vertical: bool,
        children: Vec<WidgetView>,
        sizes: Vec<u16>,
    },
    /// A PNG the plugin ships (PL§15.4), read from its package; `alt` is the
    /// accessibility label. An image that cannot be read draws as a `Text`
    /// of its `alt` instead, so a UI never sees a path.
    Image {
        alt: String,
        png: Vec<u8>,
    },
    /// `child` holds exactly one view: a list rather than a box, so the FFI
    /// mirrors the recursion as plain data.
    Block {
        title: Option<SpanView>,
        child: Vec<WidgetView>,
    },
}

/// `widget` as the app shows it: every string sanitized, and a tree over
/// PL§8's node, depth or text caps replaced by one dim line.
pub fn view(widget: &Widget) -> WidgetView {
    view_in(widget, None)
}

/// [`view`] with the images of the package at `dir`; with `None`, or an
/// image that is not a small PNG under it, the `alt` line stands in.
pub fn view_in(widget: &Widget, dir: Option<&Path>) -> WidgetView {
    if !widget.within_limits() {
        return line(TOO_LARGE.to_string(), StyleToken::Dim);
    }
    let spent = Cell::new(0);
    convert(widget, &|path| {
        let png = cox_session::plugin_ui::package_image(dir?, path).ok()?;
        spent.set(spent.get() + png.len());
        (spent.get() <= TREE_IMAGE_BYTES).then_some(png)
    })
}

/// Reads a package image by its widget path, or says it cannot be shown.
type Images<'a> = &'a dyn Fn(&str) -> Option<Vec<u8>>;

fn convert(widget: &Widget, images: Images) -> WidgetView {
    let lines = |ls: &[Vec<Span>]| -> Vec<Vec<SpanView>> {
        ls.iter().map(Vec::as_slice).map(spans).collect()
    };
    match widget {
        Widget::Text(ls) => WidgetView::Text { lines: lines(ls) },
        Widget::List { items, selected } => WidgetView::List {
            items: lines(items),
            selected: selected
                .filter(|&i| i < items.len())
                .and_then(|i| u32::try_from(i).ok()),
        },
        Widget::Table {
            header,
            rows,
            widths,
        } => {
            // PL§8 caps nodes and text, not these lists: at most one width
            // per column, so a render cannot hand the UI a million numbers.
            let columns = rows.iter().map(Vec::len).fold(header.len(), usize::max);
            WidgetView::Table {
                header: spans(header),
                rows: lines(rows),
                widths: widths.iter().take(columns).copied().collect(),
            }
        }
        Widget::KeyValue(pairs) => WidgetView::KeyValue {
            rows: pairs
                .iter()
                .map(|(key, value)| KeyValueRow {
                    key: span(key),
                    value: spans(value),
                })
                .collect(),
        },
        Widget::Gauge { ratio, label } => WidgetView::Gauge {
            ratio: if ratio.is_nan() {
                0.0
            } else {
                ratio.clamp(0.0, 1.0)
            },
            label: span(label),
        },
        Widget::Stack {
            vertical,
            children,
            sizes,
        } => WidgetView::Stack {
            vertical: *vertical,
            children: children.iter().map(|c| convert(c, images)).collect(),
            // At most one size per child, as for a table's widths.
            sizes: sizes.iter().take(children.len()).copied().collect(),
        },
        Widget::Image { path, alt } => match images(path) {
            Some(png) => WidgetView::Image {
                alt: sanitize(alt),
                png,
            },
            None => line(sanitize(alt), StyleToken::Dim),
        },
        Widget::Block { title, child } => WidgetView::Block {
            title: title.as_ref().map(span),
            child: vec![convert(child, images)],
        },
    }
}

fn spans(line: &[Span]) -> Vec<SpanView> {
    line.iter().map(span).collect()
}

fn span(s: &Span) -> SpanView {
    SpanView {
        text: sanitize(&s.text),
        style: s.style,
        bold: s.bold,
        italic: s.italic,
        // Only a link a click can follow reaches the UI, so it never draws
        // an affordance the app would then refuse.
        link: s.link.clone().filter(|l| link_target(l).is_some()),
    }
}

fn line(text: String, style: StyleToken) -> WidgetView {
    let span = SpanView {
        text,
        style,
        bold: false,
        italic: false,
        link: None,
    };
    WidgetView::Text {
        lines: vec![vec![span]],
    }
}

/// One slot as the app draws it; a patch carries the whole of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginSlot {
    pub plugin: String,
    pub slot: Slot,
    /// The last good render, kept through misses; once stopped, the
    /// "⚠ <id> slow" line.
    pub view: Option<WidgetView>,
    /// Status segments always are; a panel or overlay once shown.
    pub visible: bool,
    pub stopped: bool,
}

/// A plugin's inspector tab (PL§15.4), sanitized: a tab in the strip after
/// Info whose `Slot::DesktopInspector` renders only while it is selected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginTab {
    pub plugin: String,
    pub title: String,
    pub symbol: String,
}

/// A toolbar item or palette action (PL§15.4), sanitized. `id` is
/// `plugin:<id>:<command>`, what [`PluginUi::action`] takes and the palette
/// row carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginAction {
    pub id: String,
    pub plugin: String,
    pub title: String,
    pub symbol: String,
}

/// A request to the serve thread with the block its answer is for, if any.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub request: PluginRequest,
    pub block: Option<BlockId>,
}

/// An answer with the block its request named.
#[derive(Debug, PartialEq)]
pub struct Answered {
    pub block: Option<BlockId>,
    pub answer: PluginAnswer,
}

/// What one change to the slots gives: payloads to patch, renders to ask.
#[derive(Debug, Default, PartialEq)]
pub struct Step {
    pub slots: Vec<PluginSlot>,
    pub requests: Vec<PluginRequest>,
}

#[derive(Debug, Clone)]
struct Entry {
    plugin: String,
    slot: Slot,
    view: Option<WidgetView>,
    misses: u8,
}

impl Entry {
    fn stopped(&self) -> bool {
        self.misses >= MAX_MISSES
    }
}

/// A session's plugin slots and PL§8's redraw model: a slot renders when
/// asked, when resized and when it becomes visible, never while hidden or
/// stopped.
#[derive(Debug, Clone)]
pub struct PluginSlots {
    entries: Vec<Entry>,
    panel: Option<String>,
    overlay: Option<String>,
    /// The plugin whose inspector tab is selected; `None` while a built-in
    /// tab is, or the inspector is closed.
    inspector: Option<String>,
    area: (u16, u16),
    /// The inspector's own area: it is narrower than the window and
    /// resizes on its own.
    inspector_area: (u16, u16),
    /// Each plugin's package directory, where its widget images are read.
    dirs: BTreeMap<String, PathBuf>,
}

impl Default for PluginSlots {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            panel: None,
            overlay: None,
            inspector: None,
            area: DEFAULT_AREA,
            inspector_area: DEFAULT_AREA,
            dirs: BTreeMap::new(),
        }
    }
}

impl PluginSlots {
    /// Where `plugin`'s package is, so its widgets' images can be read.
    pub fn set_dir(&mut self, plugin: &str, dir: &Path) {
        self.dirs.insert(plugin.to_string(), dir.to_path_buf());
    }

    /// `widget` as `plugin` drew it: one guard for a slot and an item card.
    pub fn view_of(&self, plugin: &str, widget: &Widget) -> WidgetView {
        view_in(widget, self.dirs.get(plugin).map(PathBuf::as_path))
    }

    /// Registers `plugin`'s granted slots and asks for the visible ones.
    pub fn declare(&mut self, plugin: &str, slots: &[Slot]) -> Step {
        for &slot in slots {
            if !self
                .entries
                .iter()
                .any(|e| e.plugin == plugin && e.slot == slot)
            {
                self.entries.push(Entry {
                    plugin: plugin.to_string(),
                    slot,
                    view: None,
                    misses: 0,
                });
            }
        }
        self.renders(Some(plugin), None)
    }

    /// Folds one answer. A stopped slot ignores even a late render, so
    /// "slow" stays.
    pub fn fold(&mut self, answer: PluginAnswer) -> Step {
        match answer {
            PluginAnswer::Rendered {
                plugin,
                slot,
                widget,
            } => {
                let Some(i) = self.live(&plugin, slot) else {
                    return Step::default();
                };
                self.entries[i].view = Some(self.view_of(&plugin, &widget));
                self.entries[i].misses = 0;
                self.changed(&[i])
            }
            PluginAnswer::Missed { plugin, slot } => {
                let Some(i) = self.live(&plugin, slot) else {
                    return Step::default();
                };
                self.entries[i].misses += 1;
                // Until the stop the last good render stays: nothing to patch.
                match self.entries[i].stopped() {
                    true => self.changed(&[i]),
                    false => Step::default(),
                }
            }
            PluginAnswer::Redraw { plugin } => self.renders(Some(&plugin), None),
            PluginAnswer::Command { .. } | PluginAnswer::ItemRendered { .. } => Step::default(),
        }
    }

    /// The window is now `width`×`height` cells; the panel and overlay are
    /// asked again for their new area, the fixed-size segments are not.
    pub fn resize(&mut self, width: u16, height: u16) -> Step {
        if self.area == (width, height) {
            return Step::default();
        }
        self.area = (width, height);
        let mut step = self.renders(None, Some(Slot::Panel));
        step.requests
            .extend(self.renders(None, Some(Slot::Overlay)).requests);
        step
    }

    /// `CommandOut::TogglePanel`: shows `plugin`'s panel, hiding any other,
    /// or hides it.
    pub fn toggle_panel(&mut self, plugin: &str) -> Step {
        let before = self.panel.take();
        if before.as_deref() != Some(plugin) {
            self.panel = Some(plugin.to_string());
        }
        self.shown(Slot::Panel, before, plugin)
    }

    /// `CommandOut::OpenOverlay`.
    pub fn open_overlay(&mut self, plugin: &str) -> Step {
        let before = self.overlay.replace(plugin.to_string());
        self.shown(Slot::Overlay, before, plugin)
    }

    /// Esc on the overlay.
    pub fn close_overlay(&mut self) -> Step {
        match self.overlay.take() {
            Some(before) => self.shown(Slot::Overlay, Some(before.clone()), &before),
            None => Step::default(),
        }
    }

    /// The inspector now shows `plugin`'s tab, or a built-in one (`None`),
    /// `width`×`height` cells large. Only the selected tab is visible, so
    /// only it renders (PL§8's redraw model); a resize of the selected tab
    /// asks it again. A plugin that declared no tab selects nothing.
    pub fn select_tab(&mut self, plugin: Option<&str>, width: u16, height: u16) -> Step {
        let plugin = plugin.filter(|p| self.has_tab(p));
        let resized = self.inspector_area != (width, height);
        self.inspector_area = (width, height);
        let before = std::mem::replace(&mut self.inspector, plugin.map(str::to_string));
        match (before, plugin) {
            (before, Some(p)) if before.as_deref() != Some(p) => {
                self.shown(Slot::DesktopInspector, before, p)
            }
            (_, Some(p)) if resized => self.renders(Some(p), Some(Slot::DesktopInspector)),
            (Some(before), None) => self.shown(Slot::DesktopInspector, None, &before),
            _ => Step::default(),
        }
    }

    /// `CommandOut::OpenInspector`: selects `plugin`'s tab at the
    /// inspector's last area. The patch that shows it visible is what
    /// switches the app to it. A plugin without a tab changes nothing, so
    /// it cannot close another plugin's.
    pub fn open_inspector(&mut self, plugin: &str) -> Step {
        if !self.has_tab(plugin) {
            return Step::default();
        }
        let (width, height) = self.inspector_area;
        self.select_tab(Some(plugin), width, height)
    }

    fn has_tab(&self, plugin: &str) -> bool {
        self.entries
            .iter()
            .any(|e| e.plugin == plugin && e.slot == Slot::DesktopInspector)
    }

    /// The payloads of `slot` for `before` and `plugin`, whose visibility
    /// may have changed, and a render of it if it is now on screen.
    fn shown(&self, slot: Slot, before: Option<String>, plugin: &str) -> Step {
        let touched: Vec<usize> = (0..self.entries.len())
            .filter(|&i| {
                let e = &self.entries[i];
                e.slot == slot && (e.plugin == plugin || before.as_deref() == Some(&e.plugin))
            })
            .collect();
        Step {
            slots: self.changed(&touched).slots,
            requests: self.renders(Some(plugin), Some(slot)).requests,
        }
    }

    fn live(&self, plugin: &str, slot: Slot) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.plugin == plugin && e.slot == slot && !e.stopped())
    }

    fn visible(&self, e: &Entry) -> bool {
        match e.slot {
            Slot::StatusLeft | Slot::StatusRight => true,
            Slot::Panel => self.panel.as_deref() == Some(&e.plugin),
            Slot::Overlay => self.overlay.as_deref() == Some(&e.plugin),
            Slot::DesktopInspector => self.inspector.as_deref() == Some(&e.plugin),
        }
    }

    /// The area a render offers `slot`, so a plugin lays out for it.
    fn area(&self, slot: Slot) -> (u16, u16) {
        match slot {
            Slot::StatusLeft | Slot::StatusRight => (SEGMENT_COLS, 1),
            Slot::Panel => (self.area.0, PANEL_ROWS),
            Slot::Overlay => self.area,
            Slot::DesktopInspector => self.inspector_area,
        }
    }

    /// A render for every live, visible slot, narrowed to `plugin` and
    /// `slot` when given.
    fn renders(&self, plugin: Option<&str>, slot: Option<Slot>) -> Step {
        let requests = self
            .entries
            .iter()
            .filter(|e| {
                !e.stopped()
                    && plugin.is_none_or(|p| p == e.plugin)
                    && slot.is_none_or(|s| s == e.slot)
                    && self.visible(e)
            })
            .map(|e| {
                let (width, height) = self.area(e.slot);
                PluginRequest::Render {
                    plugin: e.plugin.clone(),
                    input: RenderIn {
                        slot: e.slot,
                        width,
                        height,
                    },
                }
            })
            .collect();
        Step {
            slots: Vec::new(),
            requests,
        }
    }

    fn changed(&self, indices: &[usize]) -> Step {
        let slots = indices
            .iter()
            .map(|&i| {
                let e = &self.entries[i];
                let view = match e.stopped() {
                    true => Some(line(
                        format!("⚠ {} slow", sanitize(&e.plugin)),
                        StyleToken::Warn,
                    )),
                    false => e.view.clone(),
                };
                PluginSlot {
                    plugin: e.plugin.clone(),
                    slot: e.slot,
                    view,
                    visible: self.visible(e),
                    stopped: e.stopped(),
                }
            })
            .collect();
        Step {
            slots,
            requests: Vec::new(),
        }
    }
}

/// Puts a `cox_render_item` answer on its tool or reply block. The view
/// comes from [`PluginSlots::view_of`], the one a slot's render takes, so the
/// sanitize and PL§8 limits are one guard. `None` (a miss) clears it and the
/// generic card shows.
pub fn land(kind: &mut BlockKind, widget: Option<WidgetView>) {
    if let BlockKind::Tool { plugin_view, .. } | BlockKind::Assistant { plugin_view, .. } = kind {
        *plugin_view = widget;
    }
}

/// What a plugin link names once [`is_link`] passed it (PL§15.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkTarget {
    /// An `https://` URL, normalized.
    Web(String),
    /// A workspace-relative path and the line after its `:`, if any.
    File { path: String, line: Option<u32> },
}

/// `link` as a click may follow it: the one rule of `cox_plugin_api::is_link`
/// (shared with the terminal's link marks) and a parsed URL with a host.
pub fn link_target(link: &str) -> Option<LinkTarget> {
    if !is_link(link) {
        return None;
    }
    if link.starts_with("https://") {
        let url = url::Url::parse(link).ok()?;
        return url.host().is_some().then(|| LinkTarget::Web(url.into()));
    }
    let (path, line) = match link.rsplit_once(':') {
        Some((path, line)) => (path, Some(line.parse().ok()?)),
        None => (link, None),
    };
    Some(LinkTarget::File {
        path: path.to_string(),
        line,
    })
}

/// What a click on a plugin link did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkClick {
    /// A URL went to the host (local) or to its confirmation (remote).
    Opened,
    /// A file the UI shows itself; local paths are confined, so absolute.
    File { path: String, line: Option<u32> },
    /// Not a link a click may follow; nothing happened.
    Refused,
}

/// Follows `link` as a click does: a URL goes to `open`, a path comes back
/// confined to `roots` (so a `..` that leaves the workspace is refused),
/// anything else changes nothing.
pub fn click(link: &str, roots: &[PathBuf], cwd: &Path, open: impl FnOnce(&str)) -> LinkClick {
    match link_target(link) {
        Some(LinkTarget::Web(url)) => {
            open(&url);
            LinkClick::Opened
        }
        Some(LinkTarget::File { path, line }) => cox_tools::path::confine(roots, cwd, &path)
            .map(|p| LinkClick::File {
                path: p.to_string_lossy().into_owned(),
                line,
            })
            .unwrap_or(LinkClick::Refused),
        None => LinkClick::Refused,
    }
}

/// A button on a [`PluginAlert`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertAction {
    pub label: String,
    /// `plugin:<id>:<command>`, as `plugin_action` takes it.
    pub id: String,
}

/// A plugin's desktop notice (PL§15.4) ready for the platform: every string
/// sanitized, the buttons only granted commands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginAlert {
    /// The session whose plugin posted it, which runs a button's command.
    pub session: SessionId,
    pub plugin: String,
    pub warn: bool,
    pub title: String,
    pub body: String,
    pub actions: Vec<AlertAction>,
}

/// The renderer target of an assistant reply (PL§8).
const ASSISTANT: &str = "item:assistant_message";

/// Follows a session's events for the items a plugin may render: a tool
/// call with its result, and a reply once it is whole. Separate from the
/// timeline because it keeps only what a render request needs.
#[derive(Debug, Default)]
pub struct ItemWatch {
    calls: Vec<ToolCall>,
    replies: Vec<(ItemId, String)>,
}

impl ItemWatch {
    /// The block, target and source to render, when `event` finished one.
    pub fn apply(&mut self, event: &Event) -> Option<(BlockId, String, ItemSource)> {
        match event {
            Event::ToolCallRequested { call } => self.calls.push(call.clone()),
            Event::ItemStarted {
                item,
                kind: ItemKind::AssistantMessage { text },
            } => self.replies.push((*item, text.clone())),
            Event::TextDelta { item, text } => {
                if let Some((_, whole)) = self.replies.iter_mut().find(|(i, _)| i == item) {
                    whole.push_str(text);
                }
            }
            Event::ToolCallDone { call_id, result } => {
                let at = self.calls.iter().position(|c| c.id == *call_id)?;
                let call = self.calls.swap_remove(at);
                let target = format!("tool:{}", call.name);
                let source = ItemSource::Tool {
                    call: Box::new(call),
                    result: result.clone(),
                };
                return Some((key("call", call_id), target, source));
            }
            Event::ItemDone { item } => {
                let at = self.replies.iter().position(|(i, _)| i == item)?;
                let (_, text) = self.replies.swap_remove(at);
                let source = ItemSource::Assistant { text };
                return Some((key("item", item), ASSISTANT.to_string(), source));
            }
            _ => {}
        }
        None
    }
}

/// `/<id>:<name>` with both parts sanitized, as the TUI names it.
fn full_name(plugin: &str, name: &str) -> String {
    format!("{}:{}", sanitize(plugin), sanitize(name))
}

/// `plugin`'s granted commands as completion rows.
pub fn completions(plugin: &str, commands: &[CommandDecl]) -> Vec<Completion> {
    commands
        .iter()
        .map(|c| Completion {
            insert: format!("/{}", full_name(plugin, &c.name)),
            detail: sanitize(&c.description),
        })
        .collect()
}

/// The symbol as the app may pass it to AppKit.
fn symbol(name: &str) -> String {
    match is_symbol_name(name) {
        true => name.to_string(),
        false => FALLBACK_SYMBOL.to_string(),
    }
}

/// A declared action with what its click asks: the raw plugin id and
/// command name, which the sanitized `shown` may no longer spell.
#[derive(Debug, Clone)]
struct Action {
    shown: PluginAction,
    plugin: String,
    command: String,
}

/// `decls` that name one of `commands`, at most `cap` of them; every other
/// one is left out with a line in `notices`, so a plugin's mistake is told
/// and a click can only ever reach `cox_command` with a granted command.
fn actions(
    plugin: &str,
    place: &str,
    decls: Vec<ActionDecl>,
    commands: &[CommandDecl],
    cap: usize,
    notices: &mut Vec<String>,
) -> Vec<Action> {
    let mut kept = Vec::new();
    for decl in decls {
        if !commands.iter().any(|c| c.name == decl.command) {
            notices.push(format!(
                "plugin {}: {place} action \"{}\" names no granted command \"{}\"; dropped",
                sanitize(plugin),
                sanitize(&decl.title),
                sanitize(&decl.command),
            ));
            continue;
        }
        if kept.len() == cap {
            notices.push(format!(
                "plugin {}: more than {cap} {place} actions; \"{}\" dropped",
                sanitize(plugin),
                sanitize(&decl.title),
            ));
            continue;
        }
        kept.push(Action {
            shown: PluginAction {
                id: format!("plugin:{}", full_name(plugin, &decl.command)),
                plugin: sanitize(plugin),
                title: sanitize(&decl.title),
                symbol: symbol(&decl.symbol),
            },
            plugin: plugin.to_string(),
            command: decl.command,
        });
    }
    kept
}

/// A plugin's canvas (plugin-canvas §3), sanitized: what heads the pane,
/// names it in the picker, and which attribute a click reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginCanvas {
    pub plugin: String,
    pub title: String,
    pub symbol: String,
    pub select_attr: String,
}

/// What one draw changes (§6): `order` is every frame now on the canvas,
/// `frames` only those whose page is new or changed, so an unchanged frame
/// keeps its web view and scroll offset. Each frame is capped, its title
/// sanitized; its `html` stays untrusted, for the isolated `CanvasPage`
/// (§4) only. `stopped` once [`MAX_MISSES`] draws in a row missed; the
/// canvas is not drawn again this session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CanvasUpdate {
    pub plugin: String,
    pub order: Vec<String>,
    pub frames: Vec<Frame>,
    pub stopped: bool,
}

impl PluginCanvas {
    /// `decl` as the pane heads it, or why it is left out.
    fn from_decl(plugin: &str, decl: CanvasDecl) -> Result<Self, CanvasError> {
        decl.check()?;
        Ok(Self {
            plugin: sanitize(plugin),
            title: sanitize(&decl.title),
            symbol: symbol(&decl.symbol),
            select_attr: decl.select_attr,
        })
    }

    /// One a remote server sent, checked again (plugin-canvas §7): its
    /// `select_attr` reaches the page's host script.
    pub fn checked(self) -> Option<Self> {
        let decl = CanvasDecl {
            title: self.title,
            symbol: self.symbol,
            select_attr: self.select_attr,
        };
        Self::from_decl(&self.plugin, decl).ok()
    }
}

impl CanvasUpdate {
    /// The update as a client may show it. The wire is a boundary too
    /// (plugin-canvas §7), so a remote server's frames are capped and
    /// sanitized again, and `order` keeps only the ids the caps accept.
    pub fn capped(self) -> Self {
        let mut frames = CanvasOut {
            frames: self.frames,
        }
        .capped()
        .0
        .frames;
        for f in &mut frames {
            f.title = sanitize(&f.title);
        }
        // The caps' own id rules (form, uniqueness, count), over ids alone.
        let ids = self.order.into_iter().map(|id| Frame {
            id,
            title: String::new(),
            width_pt: 0,
            html: String::new(),
        });
        let ids = CanvasOut {
            frames: ids.collect(),
        };
        Self {
            plugin: sanitize(&self.plugin),
            order: ids.capped().0.frames.into_iter().map(|f| f.id).collect(),
            frames,
            stopped: self.stopped,
        }
    }
}

/// Updates the pane has not taken yet; a full queue drops one, and the
/// next draw then sends every frame.
pub(crate) const CANVAS_QUEUE: usize = 8;

#[derive(Debug)]
struct Canvas {
    id: String,
    shown: PluginCanvas,
    visible: bool,
    size: CanvasIn,
    /// What the plugin drew last while shown: the frames a click may name.
    frames: Vec<Frame>,
    misses: u8,
}

impl Canvas {
    fn live(&self) -> bool {
        self.visible && self.misses < MAX_MISSES
    }

    /// One draw (`None` is a miss): the update to send, if anything changed,
    /// and a notice per dropped frame or for the stop.
    fn fold(
        &mut self,
        out: Option<(CanvasOut, Vec<CanvasError>)>,
    ) -> (Option<CanvasUpdate>, Vec<String>) {
        let plugin = self.shown.plugin.clone();
        let Some((out, dropped)) = out else {
            self.misses += 1;
            if self.live() {
                return (None, Vec::new());
            }
            let notice = format!("plugin {plugin}: canvas missed {MAX_MISSES} draws; stopped");
            let update = CanvasUpdate {
                plugin,
                stopped: true,
                ..CanvasUpdate::default()
            };
            return (Some(update), vec![notice]);
        };
        self.misses = 0;
        let notices = dropped
            .iter()
            .map(|e| format!("plugin {plugin}: {}", sanitize(&e.to_string())))
            .collect();
        let mut next = out.frames;
        for f in &mut next {
            f.title = sanitize(&f.title);
        }
        let order: Vec<String> = next.iter().map(|f| f.id.clone()).collect();
        let frames: Vec<Frame> = next
            .iter()
            .filter(|f| !self.frames.contains(f))
            .cloned()
            .collect();
        let same = frames.is_empty() && self.frames.iter().map(|f| &f.id).eq(&order);
        self.frames = next;
        let update = CanvasUpdate {
            plugin,
            order,
            frames,
            stopped: false,
        };
        ((!same).then_some(update), notices)
    }
}

/// The canvas plugins' hosts, which the canvas calls need outside the serve
/// thread (§8); a newtype because a `PluginHost` has no `Debug`.
#[derive(Default)]
struct Hosts(Vec<(String, Arc<PluginHost>)>);

impl fmt::Debug for Hosts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(|(id, _)| id))
            .finish()
    }
}

#[derive(Debug, Default)]
struct Declared {
    slots: PluginSlots,
    commands: Vec<(String, CommandDecl)>,
    /// `(plugin, target)` for each granted `cox_render_item` target.
    renderers: Vec<(String, String)>,
    tabs: Vec<PluginTab>,
    toolbar: Vec<Action>,
    palette: Vec<Action>,
    /// Why a declared action was left out, until the session tells it.
    notices: Vec<String>,
    canvases: Vec<Canvas>,
    hosts: Hosts,
}

impl Declared {
    /// `plugin`'s canvas while it is shown and still drawing.
    fn live_canvas(&mut self, plugin: &str) -> Option<&mut Canvas> {
        self.canvases
            .iter_mut()
            .find(|c| c.id == plugin && c.live())
    }
}

/// One session's plugin UI: its slots, its plugins' grants and the queue
/// to the serve thread (`cox_session::plugin_ui::serve`).
#[derive(Debug)]
pub struct PluginUi {
    state: Mutex<Declared>,
    requests: mpsc::Sender<Ask>,
    canvas_tx: mpsc::Sender<CanvasUpdate>,
    canvas_rx: tokio::sync::Mutex<mpsc::Receiver<CanvasUpdate>>,
}

impl PluginUi {
    pub fn new(requests: mpsc::Sender<Ask>) -> Self {
        let (canvas_tx, canvas_rx) = mpsc::channel(CANVAS_QUEUE);
        Self {
            state: Mutex::new(Declared::default()),
            requests,
            canvas_tx,
            canvas_rx: tokio::sync::Mutex::new(canvas_rx),
        }
    }

    /// Records where `plugin`'s package is, for its widgets' images.
    pub fn package(&self, plugin: &str, dir: &Path) {
        self.lock().slots.set_dir(plugin, dir);
    }

    /// `widget` as `plugin` drew it, for an item card.
    pub fn view_of(&self, plugin: &str, widget: &Widget) -> WidgetView {
        self.lock().slots.view_of(plugin, widget)
    }

    /// `notice` as an alert for `session`. A button naming a command the
    /// plugin was not granted is left out, so a click can only reach
    /// `cox_command` with one; at most [`MAX_NOTICE_ACTIONS`] stay.
    pub fn alert(&self, session: SessionId, plugin: &str, notice: DesktopNotice) -> PluginAlert {
        let d = self.lock();
        let actions = notice
            .actions
            .into_iter()
            .filter(|a| {
                d.commands
                    .iter()
                    .any(|(p, c)| p == plugin && c.name == a.command)
            })
            .take(MAX_NOTICE_ACTIONS)
            .map(|a| AlertAction {
                label: sanitize(&a.label),
                id: format!("plugin:{}", full_name(plugin, &a.command)),
            })
            .collect();
        PluginAlert {
            session,
            plugin: sanitize(plugin),
            warn: notice.level == NoticeLevel::Warn,
            title: sanitize(&notice.title),
            body: sanitize(&notice.body),
            actions,
        }
    }

    /// Every change takes the lock once, so a poisoned lock is still usable.
    fn lock(&self) -> MutexGuard<'_, Declared> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records what `plugin` was granted and asks for its visible slots.
    /// `desktop` is `InitOut.desktop` as the host left it: already empty
    /// off the desktop and without what was not granted (PL§15.6).
    pub fn declare(
        &self,
        plugin: &str,
        slots: &[Slot],
        commands: Vec<CommandDecl>,
        desktop: DesktopOut,
        renderers: Vec<String>,
    ) {
        let step = {
            let mut d = self.lock();
            let mut notices = Vec::new();
            let toolbar = actions(
                plugin,
                "toolbar",
                desktop.toolbar,
                &commands,
                MAX_TOOLBAR_ACTIONS,
                &mut notices,
            );
            let palette = actions(
                plugin,
                "palette",
                desktop.palette,
                &commands,
                usize::MAX,
                &mut notices,
            );
            d.toolbar.extend(toolbar);
            d.palette.extend(palette);
            d.notices.extend(notices);
            let mut slots = slots.to_vec();
            if let Some(tab) = desktop.inspector {
                d.tabs.push(PluginTab {
                    plugin: sanitize(plugin),
                    title: sanitize(&tab.title),
                    symbol: symbol(&tab.symbol),
                });
                slots.push(Slot::DesktopInspector);
            }
            if let Some(decl) = desktop.canvas {
                canvas(&mut d, plugin, decl);
            }
            d.commands
                .extend(commands.into_iter().map(|c| (plugin.to_string(), c)));
            d.renderers
                .extend(renderers.into_iter().map(|t| (plugin.to_string(), t)));
            d.slots.declare(plugin, &slots)
        };
        self.ask(step.requests);
    }

    /// Runs one change to the slots, sends its renders and hands back its
    /// payloads.
    pub fn step(&self, change: impl FnOnce(&mut PluginSlots) -> Step) -> Vec<PluginSlot> {
        let step = change(&mut self.lock().slots);
        self.ask(step.requests);
        step.slots
    }

    /// A full queue drops a request rather than wait: the serve thread is
    /// behind a slow plugin, and the next redraw asks again.
    fn ask(&self, requests: Vec<PluginRequest>) {
        for request in requests {
            let ask = Ask {
                request,
                block: None,
            };
            let _ = self.requests.try_send(ask);
        }
    }

    /// The granted commands as completion rows.
    pub fn completions(&self) -> Vec<Completion> {
        let d = self.lock();
        d.commands
            .iter()
            .flat_map(|(plugin, c)| completions(plugin, std::slice::from_ref(c)))
            .collect()
    }

    /// The inspector tabs, in the order the plugins started.
    pub fn tabs(&self) -> Vec<PluginTab> {
        self.lock().tabs.clone()
    }

    /// The toolbar items, at most [`MAX_TOOLBAR_ACTIONS`] per plugin.
    pub fn toolbar(&self) -> Vec<PluginAction> {
        self.lock()
            .toolbar
            .iter()
            .map(|a| a.shown.clone())
            .collect()
    }

    /// The palette actions as rows of its Actions group, each with its
    /// symbol for the well and the plugin as its detail.
    pub fn palette(&self) -> Vec<PaletteItem> {
        self.lock()
            .palette
            .iter()
            .map(|a| PaletteItem {
                kind: PaletteKind::Action,
                id: a.shown.id.clone(),
                title: a.shown.title.clone(),
                detail: a.shown.plugin.clone(),
                symbol: Some(a.shown.symbol.clone()),
            })
            .collect()
    }

    /// A toolbar item, palette action or notice button clicked: asks its
    /// plugin's `cox_command` with no arguments, whose answer lands like a
    /// typed command's. False for an id no plugin granted.
    pub fn action(&self, id: &str) -> bool {
        let found = {
            let d = self.lock();
            let declared = d
                .toolbar
                .iter()
                .chain(&d.palette)
                .find(|a| a.shown.id == id)
                .map(|a| (a.plugin.clone(), a.command.clone()));
            // A notice button may name any granted command, not only one
            // the plugin put on the toolbar.
            let named = || {
                d.commands
                    .iter()
                    .find(|(plugin, c)| id == format!("plugin:{}", full_name(plugin, &c.name)))
                    .map(|(plugin, c)| (plugin.clone(), c.name.clone()))
            };
            declared
                .or_else(named)
                .map(|(plugin, name)| PluginRequest::Command {
                    plugin,
                    name,
                    args: String::new(),
                })
        };
        let asked = found.is_some();
        self.ask(found.into_iter().collect());
        asked
    }

    /// Why declared actions were left out, once each.
    pub fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut self.lock().notices)
    }

    /// `/<id>:<name> args` for a granted command: asks the plugin and says
    /// so, so the line never reaches the core as an unknown command.
    pub fn command(&self, line: &str) -> bool {
        let Some(rest) = line.trim_start().strip_prefix('/') else {
            return false;
        };
        let (head, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let found = self
            .lock()
            .commands
            .iter()
            .find(|(plugin, c)| full_name(plugin, &c.name) == head)
            .map(|(plugin, c)| PluginRequest::Command {
                plugin: plugin.clone(),
                name: c.name.clone(),
                args: args.trim().to_string(),
            });
        let asked = found.is_some();
        self.ask(found.into_iter().collect());
        asked
    }

    /// Asks the plugin that declared `target` to render a finished item
    /// (T52.23.1, PL§8) and says whether one did. The ask names `block`, so
    /// the `ItemRendered` that answers it is [`land`]ed there.
    pub fn render_item(&self, block: BlockId, target: &str, source: ItemSource) -> bool {
        let (plugin, width) = {
            let d = self.lock();
            let found = d.renderers.iter().find(|(_, t)| t == target);
            (found.map(|(p, _)| p.clone()), d.slots.area.0)
        };
        let Some(plugin) = plugin else {
            return false;
        };
        let request = PluginRequest::RenderItem {
            plugin,
            target: target.to_string(),
            source,
            width,
        };
        let block = Some(block);
        // A full queue drops the ask, as for every other request.
        let _ = self.requests.try_send(Ask { request, block });
        true
    }
}

/// Records `plugin`'s canvas, or why it was left out: the host filtered it
/// by surface and grant, and this checks the declaration itself.
fn canvas(d: &mut Declared, plugin: &str, decl: CanvasDecl) {
    let shown = match PluginCanvas::from_decl(plugin, decl) {
        Ok(shown) => shown,
        Err(e) => {
            let why = sanitize(&e.to_string());
            let plugin = sanitize(plugin);
            d.notices
                .push(format!("plugin {plugin}: canvas left out: {why}"));
            return;
        }
    };
    d.canvases.push(Canvas {
        id: plugin.to_string(),
        shown,
        visible: false,
        size: CanvasIn {
            width_pt: 0,
            height_pt: 0,
        },
        frames: Vec::new(),
        misses: 0,
    });
}

impl PluginUi {
    /// The declared canvases, in the order their plugins started.
    pub fn canvases(&self) -> Vec<PluginCanvas> {
        self.lock()
            .canvases
            .iter()
            .map(|c| c.shown.clone())
            .collect()
    }

    /// Keeps the hosts of the plugins that declared a canvas.
    pub fn canvas_hosts(&self, hosts: Vec<(String, Arc<PluginHost>)>) {
        let mut d = self.lock();
        let kept = hosts
            .into_iter()
            .filter(|(id, _)| d.canvases.iter().any(|c| &c.id == id))
            .collect();
        d.hosts = Hosts(kept);
    }

    /// The pane shows `plugin`'s canvas at this size, newly or resized.
    /// True when it should draw now: false for an undeclared or stopped
    /// canvas. The pane starts its frames afresh, so the next draw sends
    /// them all.
    pub fn show_canvas(&self, plugin: &str, width_pt: u32, height_pt: u32) -> bool {
        let mut d = self.lock();
        let Some(c) = d.canvases.iter_mut().find(|c| c.id == plugin) else {
            return false;
        };
        c.visible = true;
        c.size = CanvasIn {
            width_pt,
            height_pt,
        };
        c.frames.clear();
        c.live()
    }

    /// The pane hid `plugin`'s canvas: nothing draws until it is shown.
    pub fn hide_canvas(&self, plugin: &str) {
        if let Some(c) = self.lock().canvases.iter_mut().find(|c| c.id == plugin) {
            c.visible = false;
        }
    }

    /// Whether a redraw of `plugin` should draw its canvas.
    pub fn canvas_live(&self, plugin: &str) -> bool {
        self.lock().live_canvas(plugin).is_some()
    }

    /// Draws `plugin`'s canvas and queues what changed for
    /// [`Self::next_canvas`]; gives the notices to tell. Blocks up to the
    /// draw's deadline, so it runs off the UI thread, and without the lock,
    /// so slots and commands go on meanwhile.
    pub fn draw_canvas(&self, plugin: &str) -> Vec<String> {
        let (input, host) = {
            let mut d = self.lock();
            let Some(c) = d.live_canvas(plugin) else {
                return Vec::new();
            };
            let input = c.size.clone();
            let host = d.hosts.0.iter().find(|(id, _)| id == plugin);
            (input, host.map(|(_, h)| Arc::clone(h)))
        };
        let out = cox_session::plugin_ui::canvas(host.as_deref(), &input);
        let mut d = self.lock();
        // Hidden while it drew: the next show draws again.
        let Some(c) = d.live_canvas(plugin) else {
            return Vec::new();
        };
        let (update, notices) = c.fold(out);
        if let Some(update) = update
            && self.canvas_tx.try_send(update).is_err()
        {
            c.frames.clear();
        }
        notices
    }

    /// The next update for the pane; `None` once the session is gone.
    pub async fn next_canvas(&self) -> Option<CanvasUpdate> {
        self.canvas_rx.lock().await.recv().await
    }

    /// A click on `frame` of `plugin`'s canvas: the plugin's attachment,
    /// sanitized and capped. A frame the plugin did not draw last is dropped
    /// before the plugin sees the click. Blocks like [`Self::draw_canvas`].
    pub fn select_canvas(&self, plugin: &str, frame: &str, value: &str) -> Option<CanvasAttach> {
        let host = {
            let mut d = self.lock();
            let drawn = d.live_canvas(plugin)?.frames.iter().any(|f| f.id == frame);
            let host = d.hosts.0.iter().find(|(id, _)| id == plugin);
            drawn.then(|| host.map(|(_, h)| Arc::clone(h)))?
        };
        let select = CanvasSelect {
            frame: frame.to_string(),
            value: value.to_string(),
        };
        cox_session::plugin_ui::canvas_select(host.as_deref(), &select).map(chip)
    }
}

/// An attachment as the composer's chip shows it: the host already capped
/// it, and both strings are the plugin's, so neither may carry escapes.
pub(crate) fn chip(attach: CanvasAttach) -> CanvasAttach {
    CanvasAttach {
        name: sanitize(&attach.name),
        text: sanitize(&attach.text),
    }
}

/// The `ServeUi` a session calls once its plugins started: the serve thread
/// over their hosts, their grants declared, and the `Redraw` their tap
/// calls, which joins every other answer on `answers`.
pub fn serve_ui(
    ui: Arc<PluginUi>,
    requests: mpsc::Receiver<Ask>,
    answers: mpsc::Sender<Answered>,
    notices: mpsc::Sender<(String, DesktopNotice)>,
) -> cox_session::ServeUi {
    Box::new(move |live: &LivePlugins| {
        // A serve thread that fails to start leaves the slots unrendered,
        // never the session unopened.
        let _ = cox_session::plugin_ui::serve(
            live.hosts(),
            requests,
            answers.clone(),
            |ask: Ask| (ask.request, ask.block),
            |block, answer| Answered { block, answer },
        );
        for p in live.plugins() {
            if let Some(dir) = p.dir() {
                ui.package(p.id(), dir);
            }
            // Leader keys are terminal-only (PL§15.3): the app has none.
            let desktop = p.init_out().map(|i| i.desktop.clone()).unwrap_or_default();
            ui.declare(
                p.id(),
                &p.granted_status(),
                p.granted_commands(),
                desktop,
                p.granted_renderers(),
            );
        }
        ui.canvas_hosts(live.hosts());
        // After the grants are declared: a notice from `cox_init` is held
        // until now, and its buttons are checked against them.
        // A full queue drops a notice: the plugin's rate limit already makes
        // a flood rare, and this runs on the plugin's own worker thread.
        let sink: cox_session::plugin_ui::DesktopNotices = Arc::new(move |plugin, notice| {
            let _ = notices.try_send((plugin.to_string(), notice));
        });
        live.on_desktop_notice(&sink);
        cox_session::plugin_ui::redraw(move |plugin| {
            // The tap's pump thread must not block on a full queue.
            let answer = PluginAnswer::Redraw { plugin };
            let _ = answers.try_send(Answered {
                block: None,
                answer,
            });
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cox_protocol::commands::COMMANDS;
    use cox_protocol::ids::CallId;
    use cox_protocol::plugin::Limits;
    use cox_protocol::plugin::ui::MAX_TEXT_BYTES;
    use cox_protocol::types::ToolResult;
    use cox_session::plugin_ui::{PluginHost, answer};

    fn text(s: &str) -> Span {
        Span {
            text: s.to_string(),
            ..Span::default()
        }
    }

    fn strings(view: &WidgetView, out: &mut Vec<String>) {
        let mut add = |spans: &[SpanView]| out.extend(spans.iter().map(|s| s.text.clone()));
        match view {
            WidgetView::Text { lines } | WidgetView::List { items: lines, .. } => {
                lines.iter().for_each(|l| add(l))
            }
            WidgetView::Table { header, rows, .. } => {
                add(header);
                rows.iter().for_each(|r| add(r));
            }
            WidgetView::KeyValue { rows } => rows.iter().for_each(|r| {
                add(std::slice::from_ref(&r.key));
                add(&r.value);
            }),
            WidgetView::Gauge { label, .. } => add(std::slice::from_ref(label)),
            WidgetView::Image { alt, .. } => out.push(alt.clone()),
            WidgetView::Stack { children, .. } => children.iter().for_each(|c| strings(c, out)),
            WidgetView::Block { title, child } => {
                add(title.as_slice());
                child.iter().for_each(|c| strings(c, out));
            }
        }
    }

    #[test]
    fn plugin_widget_strings_are_sanitized() {
        let evil = "\x1b[2J\x1b]8;;http://x\x07ok\u{202e}";
        let widget = Widget::Stack {
            vertical: true,
            children: vec![
                Widget::Text(vec![vec![text(evil)]]),
                Widget::Table {
                    header: vec![text(evil)],
                    rows: vec![vec![text(evil)]],
                    widths: vec![],
                },
                Widget::KeyValue(vec![(text(evil), vec![text(evil)])]),
                Widget::Gauge {
                    ratio: 7.0,
                    label: text(evil),
                },
                Widget::Block {
                    title: Some(text(evil)),
                    child: Box::new(Widget::List {
                        items: vec![vec![text(evil)]],
                        selected: Some(0),
                    }),
                },
            ],
            sizes: vec![],
        };
        let shown = view(&widget);
        let mut all = Vec::new();
        strings(&shown, &mut all);
        assert_eq!(all.len(), 8, "every span is carried over: {all:?}");
        for s in &all {
            assert_eq!(s, &sanitize(evil));
            assert!(!s.contains('\x1b') && !s.contains('\u{202e}'), "{s:?}");
        }
        let WidgetView::Stack { children, .. } = &shown else {
            panic!("a stack stays a stack: {shown:?}");
        };
        assert!(matches!(children[3], WidgetView::Gauge { ratio, .. } if ratio == 1.0));
    }

    #[test]
    fn plugin_widget_over_limit_is_one_line() {
        let too_large = line(TOO_LARGE.to_string(), StyleToken::Dim);
        let long = Widget::Text(vec![vec![text(&"x".repeat(MAX_TEXT_BYTES + 1))]]);
        assert_eq!(view(&long), too_large);
        let mut deep = Widget::Text(vec![]);
        for _ in 0..8 {
            deep = Widget::Block {
                title: None,
                child: Box::new(deep),
            };
        }
        assert_eq!(view(&deep), too_large, "depth 9 is over the cap");
        let wide = Widget::Stack {
            vertical: false,
            children: vec![Widget::Text(vec![]); 512],
            sizes: vec![],
        };
        assert_eq!(view(&wide), too_large, "513 nodes is over the cap");
    }

    #[test]
    fn plugin_widget_sizes_never_outnumber_their_parts() {
        let table = Widget::Table {
            header: vec![text("a"), text("b")],
            rows: vec![vec![text("1"), text("2"), text("3")]],
            widths: vec![4; 100_000],
        };
        let WidgetView::Table { widths, .. } = view(&table) else {
            panic!("a table stays a table");
        };
        assert_eq!(widths, vec![4; 3], "one width per column");
        let stack = Widget::Stack {
            vertical: true,
            children: vec![Widget::Text(vec![])],
            sizes: vec![1; 100_000],
        };
        let WidgetView::Stack { sizes, .. } = view(&stack) else {
            panic!("a stack stays a stack");
        };
        assert_eq!(sizes, vec![1], "one size per child");
        let list = Widget::List {
            items: vec![vec![text("only")]],
            selected: Some(7),
        };
        assert!(matches!(
            view(&list),
            WidgetView::List { selected: None, .. }
        ));
    }

    /// A plugin whose `cox_render` never returns (`cox_init`, the one
    /// required export, answers at once).
    fn spinning() -> PluginHost {
        let wat = r#"(module (memory 1)
          (func (export "cox_init") (result i32) (i32.const 0))
          (func (export "cox_render") (result i32) (loop $spin (br $spin)) (i32.const 0)))"#;
        // A long call cap, so only the render deadline cuts the spin short.
        let limits = Limits {
            call_ms: Some(30_000),
            ..Limits::default()
        };
        PluginHost::load("acme", wat.as_bytes(), &limits).expect("module loads")
    }

    #[test]
    fn plugin_slot_stops_after_three_misses() {
        let host = spinning();
        let mut slots = PluginSlots::default();
        let step = slots.declare("acme", &[Slot::StatusLeft]);
        assert_eq!(
            step.requests.len(),
            1,
            "a status segment is visible at once"
        );
        for miss in 1..=MAX_MISSES {
            let request = slots.renders(Some("acme"), None).requests.remove(0);
            let missed = answer(Some(&host), request);
            assert!(matches!(missed, PluginAnswer::Missed { .. }), "{missed:?}");
            let step = slots.fold(missed);
            if miss < MAX_MISSES {
                assert!(step.slots.is_empty(), "the last good render stays");
            } else {
                let stopped = &step.slots[0];
                assert!(stopped.stopped);
                assert_eq!(
                    stopped.view,
                    Some(line("⚠ acme slow".to_string(), StyleToken::Warn))
                );
            }
        }
        assert!(
            slots.renders(None, None).requests.is_empty(),
            "a stopped slot is never asked again"
        );
        let late = PluginAnswer::Rendered {
            plugin: "acme".into(),
            slot: Slot::StatusLeft,
            widget: Widget::Text(vec![vec![text("late")]]),
        };
        assert_eq!(
            slots.fold(late),
            Step::default(),
            "a late render leaves \"slow\""
        );
    }

    fn tool_block() -> BlockKind {
        BlockKind::Tool {
            tool: "acme__lint".into(),
            summary: String::new(),
            icon: crate::Icon::Shell,
            risk: cox_protocol::types::Risk::ReadOnly,
            state: crate::patch::ToolState::Done,
            tail: String::new(),
            archive: None,
            diff: None,
            duration_ms: 0,
            plugin_view: None,
        }
    }

    fn reply_block() -> BlockKind {
        BlockKind::Assistant {
            text: "hi".into(),
            doc: crate::doc::StyledDoc::default(),
            plugin_view: None,
        }
    }

    fn call(name: &str) -> ToolCall {
        ToolCall {
            id: CallId::new(),
            name: name.into(),
            input: serde_json::Value::Null,
            risk: cox_protocol::types::Risk::ReadOnly,
            subject: String::new(),
            segments: None,
        }
    }

    fn done(call_id: CallId) -> Event {
        let result = ToolResult {
            ok: true,
            visible: "ok".into(),
            archive: None,
            bytes: 2,
            duration_ms: 1,
            diff: None,
            structured: None,
        };
        Event::ToolCallDone { call_id, result }
    }

    #[test]
    fn tool_renderer_widget_lands_sanitized_in_the_block() {
        let (tx, mut rx) = mpsc::channel(4);
        let ui = PluginUi::new(tx);
        let targets = vec!["tool:acme__lint".into(), ASSISTANT.into()];
        ui.declare("acme", &[], vec![], DesktopOut::default(), targets);
        let mut watch = ItemWatch::default();
        let lint = call("acme__lint");
        let block = key("call", lint.id);
        watch.apply(&Event::ToolCallRequested { call: lint.clone() });
        let (id, target, source) = watch.apply(&done(lint.id)).expect("a finished call");
        assert_eq!(id, block);
        assert!(ui.render_item(id, &target, source));
        let ask = rx.try_recv().expect("asked");
        assert_eq!(ask.block, Some(block));
        assert!(
            matches!(ask.request, PluginRequest::RenderItem { ref plugin, ref target, .. }
            if plugin == "acme" && target == "tool:acme__lint")
        );
        let read = call("read");
        watch.apply(&Event::ToolCallRequested { call: read.clone() });
        let (id, target, source) = watch.apply(&done(read.id)).expect("a finished call");
        assert!(!ui.render_item(id, &target, source), "no renderer, no ask");

        let widget = Widget::Text(vec![vec![text("\x1b[2Jdone")]]);
        let mut kind = tool_block();
        land(&mut kind, Some(view(&widget)));
        let BlockKind::Tool { plugin_view, .. } = &kind else {
            panic!("a tool stays a tool");
        };
        assert_eq!(plugin_view, &Some(view(&widget)));
        let mut all = Vec::new();
        strings(plugin_view.as_ref().expect("landed"), &mut all);
        assert_eq!(all, vec!["done".to_string()]);
        land(&mut kind, None);
        assert!(matches!(
            kind,
            BlockKind::Tool {
                plugin_view: None,
                ..
            }
        ));
    }

    #[test]
    fn item_renderer_widget_lands_sanitized_in_the_reply() {
        let (tx, mut rx) = mpsc::channel(4);
        let ui = PluginUi::new(tx);
        ui.declare(
            "acme",
            &[],
            vec![],
            DesktopOut::default(),
            vec![ASSISTANT.into()],
        );
        let mut watch = ItemWatch::default();
        let item = ItemId::new();
        let started = ItemKind::AssistantMessage { text: "he".into() };
        watch.apply(&Event::ItemStarted {
            item,
            kind: started,
        });
        watch.apply(&Event::TextDelta {
            item,
            text: "llo".into(),
        });
        let (id, target, source) = watch.apply(&Event::ItemDone { item }).expect("a reply");
        assert_eq!(id, key("item", item));
        assert!(matches!(&source, ItemSource::Assistant { text } if text == "hello"));
        assert!(ui.render_item(id.clone(), &target, source));
        assert_eq!(rx.try_recv().expect("asked").block, Some(id));

        let widget = Widget::Text(vec![vec![text("\x1b]8;;http://x\x07hi\u{202e}")]]);
        let mut kind = reply_block();
        land(&mut kind, Some(view(&widget)));
        let BlockKind::Assistant { plugin_view, .. } = &kind else {
            panic!("a reply stays a reply");
        };
        let mut all = Vec::new();
        strings(plugin_view.as_ref().expect("landed"), &mut all);
        assert_eq!(all, vec!["hi".to_string()]);
    }

    #[test]
    fn plugin_command_never_shadows_a_builtin() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("cox-home");
        let mut completer = crate::Completer::load(dir.path(), &home, &dir.path().join("claude"));
        let compact = CommandDecl {
            name: "compact".into(),
            description: "\x1b[31mmine\x1b[0m".into(),
        };
        let rows = completions("acme", &[compact]);
        completer.extend(rows.clone());
        completer.extend(rows);
        let all = completer.complete("/", 500);
        assert_eq!(
            all.len(),
            COMMANDS.len() + 1,
            "added once, after the built-ins"
        );
        assert_eq!(all[COMMANDS.len()].insert, "/acme:compact");
        assert_eq!(all[COMMANDS.len()].detail, "mine");
        assert_eq!(completer.complete("/compact", 1)[0].insert, "/compact");

        let (tx, mut rx) = mpsc::channel(4);
        let ui = PluginUi::new(tx);
        let decl = CommandDecl {
            name: "compact".into(),
            description: String::new(),
        };
        ui.declare("acme", &[], vec![decl], DesktopOut::default(), vec![]);
        assert!(!ui.command("/compact now"), "the built-in stays the core's");
        assert!(ui.command("/acme:compact now"));
        assert_eq!(
            rx.try_recv().ok(),
            Some(Ask {
                request: PluginRequest::Command {
                    plugin: "acme".into(),
                    name: "compact".into(),
                    args: "now".into(),
                },
                block: None,
            })
        );
    }

    fn inspector_render(width: u16, height: u16) -> PluginRequest {
        PluginRequest::Render {
            plugin: "acme".into(),
            input: RenderIn {
                slot: Slot::DesktopInspector,
                width,
                height,
            },
        }
    }

    #[test]
    fn inspector_tab_renders_only_while_selected() {
        let mut slots = PluginSlots::default();
        let step = slots.declare("acme", &[Slot::DesktopInspector]);
        assert!(step.requests.is_empty(), "an unselected tab is not asked");
        let redraw = || PluginAnswer::Redraw {
            plugin: "acme".into(),
        };
        assert!(slots.fold(redraw()).requests.is_empty());

        let step = slots.select_tab(Some("acme"), 40, 30);
        assert_eq!(step.requests, vec![inspector_render(40, 30)]);
        assert!(step.slots.iter().all(|s| s.visible), "{:?}", step.slots);
        assert_eq!(
            slots.fold(redraw()).requests,
            vec![inspector_render(40, 30)]
        );
        assert_eq!(
            slots.select_tab(Some("acme"), 50, 30).requests,
            vec![inspector_render(50, 30)],
            "a resized tab is asked again"
        );
        assert!(
            slots.resize(120, 40).requests.is_empty(),
            "the window's size is not the inspector's"
        );

        let step = slots.select_tab(None, 50, 30);
        assert!(step.requests.is_empty());
        assert!(step.slots.iter().all(|s| !s.visible), "{:?}", step.slots);
        assert!(slots.fold(redraw()).requests.is_empty(), "hidden again");

        assert_eq!(
            slots.open_inspector("acme").requests,
            vec![inspector_render(50, 30)],
            "OpenInspector selects the tab at its last size"
        );
        assert_eq!(slots.open_inspector("other"), Step::default());
        assert!(
            slots
                .select_tab(Some("other"), 50, 30)
                .slots
                .iter()
                .all(|s| !s.visible),
            "a plugin without a tab selects none and hides acme's"
        );
    }

    fn command(name: &str) -> CommandDecl {
        CommandDecl {
            name: name.into(),
            description: String::new(),
        }
    }

    fn act(command: &str, title: &str, symbol: &str) -> ActionDecl {
        ActionDecl {
            command: command.into(),
            title: title.into(),
            symbol: symbol.into(),
        }
    }

    #[test]
    fn plugin_action_runs_its_command() {
        let (tx, mut rx) = mpsc::channel(4);
        let ui = PluginUi::new(tx);
        let desktop = DesktopOut {
            inspector: Some(cox_protocol::plugin::TabDecl {
                title: "\x1b[31mGlance".into(),
                symbol: "eye".into(),
            }),
            toolbar: vec![act("go", "Go\u{202e}", "play.fill")],
            palette: vec![act("go", "Go now", "Not A Symbol")],
            canvas: None,
        };
        ui.declare("acme", &[], vec![command("go")], desktop, vec![]);
        assert!(ui.take_notices().is_empty());
        assert_eq!(
            ui.tabs(),
            vec![PluginTab {
                plugin: "acme".into(),
                title: "Glance".into(),
                symbol: "eye".into(),
            }]
        );
        let toolbar = ui.toolbar();
        assert_eq!(toolbar.len(), 1);
        assert_eq!(toolbar[0].id, "plugin:acme:go");
        assert_eq!(toolbar[0].title, sanitize("Go\u{202e}"));
        let rows = ui.palette();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, PaletteKind::Action);
        assert_eq!(rows[0].symbol.as_deref(), Some(FALLBACK_SYMBOL));
        let hits = crate::palette::rank("go now", rows.clone(), 5);
        assert_eq!(hits[0].item.id, "plugin:acme:go");

        assert!(ui.action(&rows[0].id));
        assert_eq!(
            rx.try_recv().ok(),
            Some(Ask {
                request: PluginRequest::Command {
                    plugin: "acme".into(),
                    name: "go".into(),
                    args: String::new(),
                },
                block: None,
            })
        );
        assert!(!ui.action("plugin:acme:stop"), "never declared");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn plugin_action_naming_an_undeclared_command_is_dropped() {
        let (tx, _rx) = mpsc::channel(4);
        let ui = PluginUi::new(tx);
        let desktop = DesktopOut {
            inspector: None,
            toolbar: vec![
                act("missing", "Ghost", "eye"),
                act("a", "A", "eye"),
                act("b", "B", "eye"),
                act("c", "C", "eye"),
            ],
            palette: vec![act("missing", "Ghost", "eye")],
            canvas: None,
        };
        let commands = vec![command("a"), command("b"), command("c")];
        ui.declare("acme", &[], commands, desktop, vec![]);
        let ids: Vec<String> = ui.toolbar().into_iter().map(|a| a.id).collect();
        assert_eq!(ids, ["plugin:acme:a", "plugin:acme:b"], "at most two");
        assert!(ui.palette().is_empty());
        assert!(!ui.action("plugin:acme:missing"));
        let notices = ui.take_notices();
        assert_eq!(notices.len(), 3, "{notices:?}");
        assert!(notices[0].contains("\"missing\""), "{notices:?}");
        assert!(notices[1].contains("\"C\" dropped"), "{notices:?}");
        assert!(ui.take_notices().is_empty(), "told once");
        assert!(ui.tabs().is_empty());
    }

    fn png(len: usize) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.resize(len, 0);
        bytes
    }

    fn image(path: &str) -> Widget {
        Widget::Image {
            path: path.into(),
            alt: "\x1b[2Jchart".into(),
        }
    }

    #[test]
    fn an_image_is_read_from_the_package_and_otherwise_draws_its_alt_text() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.png"), png(64)).expect("png");
        std::fs::write(dir.path().join("b.png"), b"not a png").expect("text");
        let at = |w: &Widget| view_in(w, Some(dir.path()));
        assert_eq!(
            at(&image("a.png")),
            WidgetView::Image {
                alt: "chart".into(),
                png: png(64)
            }
        );
        let alt = line("chart".into(), StyleToken::Dim);
        for path in ["b.png", "missing.png", "../a.png", "/etc/hosts", "c:a.png"] {
            assert_eq!(at(&image(path)), alt, "{path}");
        }
        assert_eq!(view(&image("a.png")), alt, "no package, no image");
    }

    #[cfg(unix)]
    #[test]
    fn an_image_symlinked_out_of_the_package_is_not_read() {
        let (dir, other) = (
            tempfile::tempdir().expect("dir"),
            tempfile::tempdir().expect("dir"),
        );
        std::fs::write(other.path().join("secret.png"), png(64)).expect("png");
        std::os::unix::fs::symlink(other.path().join("secret.png"), dir.path().join("a.png"))
            .expect("link");
        let shown = view_in(&image("a.png"), Some(dir.path()));
        assert!(matches!(shown, WidgetView::Text { .. }), "{shown:?}");
    }

    #[test]
    fn one_tree_cannot_carry_more_image_bytes_than_its_budget() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("big.png"), png(MAX_IMAGE_BYTES)).expect("png");
        let tree = Widget::Stack {
            vertical: true,
            children: (0..5).map(|_| image("big.png")).collect(),
            sizes: vec![],
        };
        let WidgetView::Stack { children, .. } = view_in(&tree, Some(dir.path())) else {
            panic!("a stack stays a stack");
        };
        let drawn = children
            .iter()
            .filter(|c| matches!(c, WidgetView::Image { .. }))
            .count();
        assert_eq!(drawn, TREE_IMAGE_BYTES / MAX_IMAGE_BYTES);
    }

    #[test]
    fn a_span_link_reaches_the_ui_only_when_a_click_may_follow_it() {
        let linked = |l: &str| {
            span(&Span {
                text: "x".into(),
                link: Some(l.into()),
                ..Span::default()
            })
            .link
        };
        assert_eq!(
            linked("https://example.com/a"),
            Some("https://example.com/a".into())
        );
        assert_eq!(linked("src/lib.rs:12"), Some("src/lib.rs:12".into()));
        for bad in [
            "http://example.com",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "",
        ] {
            assert_eq!(linked(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_click_opens_a_web_link_and_confines_a_path_to_the_workspace() {
        let root = tempfile::tempdir().expect("root");
        std::fs::create_dir(root.path().join("src")).expect("src");
        std::fs::write(root.path().join("src/lib.rs"), "").expect("file");
        let roots = vec![root.path().to_path_buf()];
        let opened = std::cell::RefCell::new(Vec::new());
        let go = |link: &str| {
            click(link, &roots, root.path(), |url| {
                opened.borrow_mut().push(url.to_string())
            })
        };
        assert_eq!(go("https://example.com"), LinkClick::Opened);
        assert_eq!(*opened.borrow(), ["https://example.com/"]);
        let LinkClick::File { path, line } = go("src/lib.rs:7") else {
            panic!("a workspace path is a file");
        };
        assert!(path.ends_with("src/lib.rs"), "{path}");
        assert_eq!(line, Some(7));
        for bad in [
            "../outside",
            "/etc/passwd",
            "http://example.com",
            "file:///etc/passwd",
            "a:b:c",
        ] {
            assert_eq!(go(bad), LinkClick::Refused, "{bad}");
        }
        assert_eq!(opened.borrow().len(), 1, "a refused link opened nothing");
    }

    fn notice(actions: &[(&str, &str)]) -> DesktopNotice {
        DesktopNotice {
            level: NoticeLevel::Warn,
            title: "\x1b[2JBreak\u{202e}".into(),
            body: "Stand up".into(),
            actions: actions
                .iter()
                .map(|(label, command)| cox_protocol::plugin::NoticeAction {
                    label: (*label).into(),
                    command: (*command).into(),
                })
                .collect(),
        }
    }

    #[test]
    fn a_notice_button_runs_a_granted_command_and_an_ungranted_one_is_dropped() {
        let (tx, mut rx) = mpsc::channel(4);
        let ui = PluginUi::new(tx);
        ui.declare(
            "acme",
            &[],
            vec![command("snooze")],
            DesktopOut::default(),
            vec![],
        );
        let session = SessionId::new();
        let alert = ui.alert(
            session,
            "acme",
            notice(&[
                ("Snooze\x1b[31m", "snooze"),
                ("Ghost", "ghost"),
                ("Other", "snooze"),
            ]),
        );
        assert_eq!(alert.session, session);
        assert_eq!(alert.title, sanitize("\x1b[2JBreak\u{202e}"));
        assert!(alert.warn);
        let ids: Vec<&str> = alert.actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["plugin:acme:snooze", "plugin:acme:snooze"]);
        assert_eq!(alert.actions[0].label, "Snooze");

        assert!(
            ui.action(&alert.actions[0].id),
            "not on the toolbar, but granted"
        );
        assert_eq!(
            rx.try_recv().ok().map(|a| a.request),
            Some(PluginRequest::Command {
                plugin: "acme".into(),
                name: "snooze".into(),
                args: String::new(),
            })
        );
        assert!(!ui.action("plugin:acme:ghost"));
        assert!(!ui.action("plugin:other:snooze"));
        assert!(rx.try_recv().is_err());
    }

    /// A plugin whose canvas exports never return, under a `call_ms` of
    /// `call_ms`, so a canvas call ends at the shorter of it and its own
    /// deadline.
    fn spinning_canvas(call_ms: u32) -> PluginHost {
        let wat = r#"(module (memory 1)
          (func (export "cox_init") (result i32) (i32.const 0))
          (func (export "cox_canvas") (result i32) (loop $spin (br $spin)) (i32.const 0))
          (func (export "cox_canvas_select") (result i32) (loop $spin (br $spin)) (i32.const 0)))"#;
        let limits = Limits {
            call_ms: Some(call_ms),
            ..Limits::default()
        };
        PluginHost::load("acme", wat.as_bytes(), &limits).expect("module loads")
    }

    fn canvas_ui(host: Option<PluginHost>) -> PluginUi {
        let ui = PluginUi::new(mpsc::channel(8).0);
        let desktop = DesktopOut {
            canvas: Some(CanvasDecl {
                title: "Screens".into(),
                symbol: "rectangle.on.rectangle".into(),
                select_attr: "data-weft-path".into(),
            }),
            ..DesktopOut::default()
        };
        ui.declare("acme", &[], Vec::new(), desktop, Vec::new());
        let hosts = host.map(|h| ("acme".to_string(), Arc::new(h)));
        ui.canvas_hosts(hosts.into_iter().collect());
        ui
    }

    fn page(id: &str, title: &str, html: &str) -> Frame {
        Frame {
            id: id.into(),
            title: title.into(),
            width_pt: 390,
            html: html.into(),
        }
    }

    fn update(ui: &PluginUi) -> Option<CanvasUpdate> {
        ui.canvas_rx.try_lock().expect("unshared").try_recv().ok()
    }

    /// plugin-canvas §2, §6: frames come back capped and sanitized, and a
    /// draw sends only the frames whose page changed.
    #[test]
    fn a_canvas_sends_only_changed_frames() {
        let ui = canvas_ui(None);
        assert!(ui.show_canvas("acme", 800, 600));
        let draw = |frames: Vec<Frame>| {
            let mut d = ui.lock();
            let c = d.live_canvas("acme").expect("shown");
            c.fold(Some(CanvasOut { frames }.capped()))
        };
        let (first, notices) = draw(vec![
            page("home", "Home\u{1b}[2J", "<p>1</p>"),
            page("Bad!", "x", ""),
            page("list", "List", "<p>2</p>"),
        ]);
        let first = first.expect("an update");
        assert_eq!(first.order, ["home", "list"]);
        assert_eq!(first.frames[0].title, "Home");
        assert_eq!(notices.len(), 1, "the bad id is told: {notices:?}");
        let same = vec![
            page("home", "Home", "<p>1</p>"),
            page("list", "List", "<p>2</p>"),
        ];
        assert_eq!(draw(same.clone()).0, None, "nothing changed");
        let mut changed = same;
        changed[1].html = "<p>3</p>".into();
        let next = draw(changed).0.expect("an update");
        assert_eq!(next.order, ["home", "list"]);
        assert_eq!(next.frames.len(), 1);
        assert_eq!(next.frames[0].id, "list");
    }

    /// §6, PL§8: a hidden canvas never draws, and three slow draws in a row
    /// stop it for the session.
    #[test]
    fn a_slow_canvas_stops_after_three_misses() {
        let ui = canvas_ui(Some(spinning_canvas(50)));
        assert_eq!(ui.canvases()[0].select_attr, "data-weft-path");
        assert!(!ui.canvas_live("acme"), "hidden until the pane shows it");
        assert!(ui.draw_canvas("acme").is_empty());
        assert_eq!(update(&ui), None);
        assert!(ui.show_canvas("acme", 800, 600));
        for miss in 1..=MAX_MISSES {
            let notices = ui.draw_canvas("acme");
            assert_eq!(notices.len(), usize::from(miss == MAX_MISSES));
        }
        let stopped = update(&ui).expect("the stop is sent");
        assert!(stopped.stopped);
        assert!(!ui.canvas_live("acme"));
        assert!(!ui.show_canvas("acme", 800, 600), "never drawn again");
        ui.hide_canvas("acme");
    }

    /// §5: a click on a frame the plugin did not draw, or with a value the
    /// caps refuse, never reaches the plugin; a drawn frame does.
    #[test]
    fn a_click_reaches_the_plugin_only_on_a_drawn_frame() {
        let ui = canvas_ui(Some(spinning_canvas(30_000)));
        assert!(ui.show_canvas("acme", 800, 600));
        let frames = vec![page("home", "Home", "<p>1</p>")];
        let mut d = ui.lock();
        let c = d.live_canvas("acme").expect("shown");
        let _ = c.fold(Some(CanvasOut { frames }.capped()));
        drop(d);
        let quick = |frame: &str, value: &str| {
            let started = std::time::Instant::now();
            assert_eq!(ui.select_canvas("acme", frame, value), None);
            started.elapsed() < std::time::Duration::from_millis(300)
        };
        assert!(quick("other", "hero"), "an unknown frame is dropped");
        assert!(quick("home", "a\u{7}b"), "a bad value is dropped");
        assert!(!quick("home", "hero"), "a drawn frame waits on the plugin");
    }

    /// §5: the chip's name and text carry no escapes.
    #[test]
    fn a_canvas_attachment_is_sanitized() {
        let shown = chip(CanvasAttach {
            name: "Hero\u{1b}]8;;x\u{7}".into(),
            text: "screen home\nnode hero\u{202e}".into(),
        });
        assert_eq!(shown.name, "Hero");
        assert_eq!(shown.text, "screen home\nnode hero");
    }
}
