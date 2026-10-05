// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The desktop app's side of plugin UI (T52.14, PL§8): the closed `Widget`
//! tree a plugin renders, turned into a [`WidgetView`] whose every string went
//! through `cox_sanitize` and whose size PL§8 caps, and the per-session slot
//! state — which slots are on screen, what area each gets, when each renders
//! again and when three missed deadlines stop one. Separate from
//! `cox-session`'s `plugin_ui`, which only calls the hosts: this module is
//! what the app shows, as `cox-tui`'s `status.rs` is for the terminal.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use cox_protocol::ids::ItemId;
use cox_protocol::plugin::ui::Span;
use cox_protocol::plugin::{CommandDecl, KeyDecl, RenderIn, Slot, StyleToken, Widget};
use cox_protocol::types::{Event, ItemKind, ToolCall};
use cox_sanitize::sanitize;
use cox_session::plugin_ui::{ItemSource, LivePlugins, PluginAnswer, PluginRequest};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::Completion;
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

/// A run of sanitized text with one colour role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanView {
    pub text: String,
    pub style: StyleToken,
    pub bold: bool,
    pub italic: bool,
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
    if widget.within_limits() {
        convert(widget)
    } else {
        line(TOO_LARGE.to_string(), StyleToken::Dim)
    }
}

fn convert(widget: &Widget) -> WidgetView {
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
            children: children.iter().map(convert).collect(),
            // At most one size per child, as for a table's widths.
            sizes: sizes.iter().take(children.len()).copied().collect(),
        },
        Widget::Block { title, child } => WidgetView::Block {
            title: title.as_ref().map(span),
            child: vec![convert(child)],
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
    }
}

fn line(text: String, style: StyleToken) -> WidgetView {
    let span = SpanView {
        text,
        style,
        bold: false,
        italic: false,
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

/// A granted key under the plugin leader, sanitized for the help list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginKey {
    pub plugin: String,
    pub key: String,
    pub name: String,
    pub description: String,
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
    area: (u16, u16),
}

impl Default for PluginSlots {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            panel: None,
            overlay: None,
            area: DEFAULT_AREA,
        }
    }
}

impl PluginSlots {
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
                self.entries[i].view = Some(view(&widget));
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
        }
    }

    /// The area a render offers `slot`, so a plugin lays out for it.
    fn area(&self, slot: Slot) -> (u16, u16) {
        match slot {
            Slot::StatusLeft | Slot::StatusRight => (SEGMENT_COLS, 1),
            Slot::Panel => (self.area.0, PANEL_ROWS),
            Slot::Overlay => self.area,
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

/// Puts a `cox_render_item` answer on its tool or reply block through the same
/// [`view`] a slot's render takes, so the sanitize and PL§8 limits are the
/// one guard. `None` (a miss) clears it and the generic card shows.
pub fn land(kind: &mut BlockKind, widget: Option<&Widget>) {
    if let BlockKind::Tool { plugin_view, .. } | BlockKind::Assistant { plugin_view, .. } = kind {
        *plugin_view = widget.map(view);
    }
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

#[derive(Debug, Default)]
struct Declared {
    slots: PluginSlots,
    commands: Vec<(String, CommandDecl)>,
    keys: Vec<(String, KeyDecl)>,
    /// `(plugin, target)` for each granted `cox_render_item` target.
    renderers: Vec<(String, String)>,
}

/// One session's plugin UI: its slots, its plugins' grants and the queue
/// to the serve thread (`cox_session::plugin_ui::serve`).
#[derive(Debug)]
pub struct PluginUi {
    state: Mutex<Declared>,
    requests: mpsc::Sender<Ask>,
}

impl PluginUi {
    pub fn new(requests: mpsc::Sender<Ask>) -> Self {
        Self {
            state: Mutex::new(Declared::default()),
            requests,
        }
    }

    /// Every change takes the lock once, so a poisoned lock is still usable.
    fn lock(&self) -> MutexGuard<'_, Declared> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records what `plugin` was granted and asks for its visible slots.
    pub fn declare(
        &self,
        plugin: &str,
        slots: &[Slot],
        commands: Vec<CommandDecl>,
        keys: Vec<KeyDecl>,
        renderers: Vec<String>,
    ) {
        let step = {
            let mut d = self.lock();
            d.commands
                .extend(commands.into_iter().map(|c| (plugin.to_string(), c)));
            d.keys
                .extend(keys.into_iter().map(|k| (plugin.to_string(), k)));
            d.renderers
                .extend(renderers.into_iter().map(|t| (plugin.to_string(), t)));
            d.slots.declare(plugin, slots)
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

    /// The granted keys, sanitized.
    pub fn keys(&self) -> Vec<PluginKey> {
        let d = self.lock();
        d.keys
            .iter()
            .map(|(plugin, k)| PluginKey {
                plugin: sanitize(plugin),
                key: sanitize(&k.key),
                name: sanitize(&k.name),
                description: sanitize(&k.description),
            })
            .collect()
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

    /// The key `name` (as [`PluginKey`] shows it) of `plugin`, if granted.
    pub fn key(&self, plugin: &str, name: &str) -> bool {
        let found = self
            .lock()
            .keys
            .iter()
            .find(|(p, k)| p == plugin && sanitize(&k.name) == name)
            .map(|(p, k)| PluginRequest::Key {
                plugin: p.clone(),
                name: k.name.clone(),
            });
        let asked = found.is_some();
        self.ask(found.into_iter().collect());
        asked
    }
}

/// The `ServeUi` a session calls once its plugins started: the serve thread
/// over their hosts, their grants declared, and the `Redraw` their tap
/// calls, which joins every other answer on `answers`.
pub fn serve_ui(
    ui: Arc<PluginUi>,
    requests: mpsc::Receiver<Ask>,
    answers: mpsc::Sender<Answered>,
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
            ui.declare(
                p.id(),
                &p.granted_status(),
                p.granted_commands(),
                p.granted_keys(),
                p.granted_renderers(),
            );
        }
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
        ui.declare("acme", &[], vec![], vec![], targets);
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
        land(&mut kind, Some(&widget));
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
        ui.declare("acme", &[], vec![], vec![], vec![ASSISTANT.into()]);
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
        land(&mut kind, Some(&widget));
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
        ui.declare("acme", &[], vec![decl], vec![], vec![]);
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
}
