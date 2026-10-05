// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Plugin renderers for finished cells (T33.26, PL§8): a `tool:<name>` or
//! `item:assistant_message` target is asked once, when its cell completes,
//! and the answer is cached in the cell. Its own module so `state` only
//! calls in at the four points that matter (declare, cell done, answer,
//! tick). The widget lives only in `Cell`: the core never sees it, so it
//! cannot reach the rollout, the archive or the next model `Request`.

use cox_protocol::ids::{CallId, ItemId};
use cox_protocol::plugin::Widget;
use cox_protocol::types::{ToolCall, ToolResult};

use crate::state::{Cell, Cmd, PluginRequest, State};

/// The one non-tool target (PL§8).
pub const ASSISTANT: &str = "item:assistant_message";

/// Ticks (100 ms each) a finished cell waits for its render before the
/// built-in one takes over. The host already folds its own 20 ms deadline
/// into a `None` answer; this covers a request `app.rs` could not queue,
/// which never answers at all, so a cell can never hold scrollback forever.
pub const WAIT_TICKS: u64 = 5;

/// How a finished `Cell::Tool` or `Cell::Assistant` is drawn.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum ItemRender {
    /// `cells::cell_lines`' own rendering: no renderer, or it fell back.
    #[default]
    Builtin,
    /// Asked at `State::tick` `since`; the cell is not `done` meanwhile, so
    /// it stays in the viewport instead of reaching scrollback unrendered.
    Pending { since: u64 },
    /// The plugin's answer, drawn through `plugin_ui` (the sanitize boundary).
    Plugin(Box<Widget>),
}

impl ItemRender {
    pub fn pending(&self) -> bool {
        matches!(self, ItemRender::Pending { .. })
    }
}

/// Which cell a render belongs to: its call or its item id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellRef {
    Call(CallId),
    Item(ItemId),
}

/// What `cox_render_item` renders. `crates/cox` turns it into
/// `RenderItemIn`, since `cox-tui` has no JSON of its own.
#[derive(Debug, Clone, PartialEq)]
pub enum RenderSource {
    Tool {
        call: Box<ToolCall>,
        result: ToolResult,
    },
    Assistant {
        text: String,
    },
}

/// `plugin`'s granted targets after `cox_init`: the current, full set.
pub fn declare(state: &mut State, plugin: &str, targets: &[String]) {
    let own = &mut state.plugin_renderers;
    own.retain(|(p, _)| p != plugin);
    own.extend(targets.iter().map(|t| (plugin.to_string(), t.clone())));
}

/// `cell` just completed: asks its renderer, if one is declared, and holds
/// the cell until the answer or `WAIT_TICKS`.
pub fn ask(state: &mut State, cell: CellRef) -> Vec<Cmd> {
    let (target, source) = match find(state, cell) {
        Some(Cell::Tool {
            call,
            result: Some(result),
            ..
        }) => (
            format!("tool:{}", call.name),
            RenderSource::Tool {
                call: call.clone(),
                result: result.clone(),
            },
        ),
        Some(Cell::Assistant {
            text, done: true, ..
        }) => (
            ASSISTANT.to_string(),
            RenderSource::Assistant { text: text.clone() },
        ),
        _ => return Vec::new(),
    };
    let Some(plugin) = state
        .plugin_renderers
        .iter()
        .find(|(_, t)| *t == target)
        .map(|(p, _)| p.clone())
    else {
        return Vec::new();
    };
    let since = state.tick;
    let width = state.term.0;
    if let Some(render) = find(state, cell).and_then(render_mut) {
        *render = ItemRender::Pending { since };
    }
    vec![Cmd::Plugin(PluginRequest::RenderItem {
        plugin,
        cell,
        target,
        source,
        width,
    })]
}

/// A `cox_render_item` answer; `None` (a timeout, an error, a missing
/// export) keeps the built-in rendering. Late answers are ignored, so a
/// cell already in scrollback never changes.
pub fn on_rendered(state: &mut State, cell: CellRef, widget: Option<Widget>) {
    if let Some(render) = find(state, cell).and_then(render_mut)
        && render.pending()
    {
        *render = widget.map_or(ItemRender::Builtin, |w| ItemRender::Plugin(Box::new(w)));
    }
}

/// Falls back every render still pending after `WAIT_TICKS`.
pub fn expire(state: &mut State) {
    let now = state.tick;
    for render in state.transcript.iter_mut().filter_map(render_mut) {
        if let ItemRender::Pending { since } = render
            && now.saturating_sub(*since) >= WAIT_TICKS
        {
            *render = ItemRender::Builtin;
        }
    }
}

fn find(state: &mut State, cell: CellRef) -> Option<&mut Cell> {
    state.transcript.iter_mut().rev().find(|c| match (c, cell) {
        (Cell::Tool { call, .. }, CellRef::Call(id)) => call.id == id,
        (Cell::Assistant { item, .. }, CellRef::Item(id)) => *item == id,
        _ => false,
    })
}

fn render_mut(cell: &mut Cell) -> Option<&mut ItemRender> {
    match cell {
        Cell::Tool { render, .. } | Cell::Assistant { render, .. } => Some(render),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use cox_protocol::plugin::ui::{Span, StyleToken};
    use cox_protocol::types::{Event, PermissionMode, Risk, SandboxMode};

    use super::*;
    use crate::cells::cell_lines;
    use crate::state::{Msg, PluginUiMsg, update};

    fn fresh(renderers: &[&str]) -> State {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.still = true;
        let declare = PluginUiMsg::Declare {
            plugin: "look".into(),
            slots: Vec::new(),
            commands: Vec::new(),
            keys: Vec::new(),
            renderers: renderers.iter().map(|t| t.to_string()).collect(),
        };
        update(&mut state, Msg::Plugin(declare));
        state
    }

    /// A finished `read` call and a finished reply; the commands they raised.
    fn finish_both(state: &mut State) -> Vec<Cmd> {
        let id = CallId::new();
        let call = ToolCall {
            id,
            name: "read".into(),
            input: serde_json::json!({ "path": "src/lib.rs" }),
            risk: Risk::ReadOnly,
            subject: "src/lib.rs".into(),
            segments: None,
        };
        let result = ToolResult {
            ok: true,
            visible: "fn main() {}".into(),
            archive: None,
            bytes: 12,
            duration_ms: 7,
            diff: None,
            structured: None,
        };
        let item = ItemId::new();
        let text = "Plain **markdown** reply.".to_string();
        let events = [
            Event::ToolCallRequested { call },
            Event::ToolCallOutput {
                call_id: id,
                delta: "fn main() {}".into(),
            },
            Event::ToolCallDone {
                call_id: id,
                result,
            },
            Event::ItemStarted {
                item,
                kind: cox_protocol::types::ItemKind::AssistantMessage { text },
            },
            Event::ItemDone { item },
        ];
        events
            .into_iter()
            .flat_map(|ev| update(state, Msg::Event(ev)))
            .collect()
    }

    fn requests(cmds: &[Cmd]) -> Vec<(CellRef, String)> {
        cmds.iter()
            .filter_map(|c| match c {
                Cmd::Plugin(PluginRequest::RenderItem { cell, target, .. }) => {
                    Some((*cell, target.clone()))
                }
                _ => None,
            })
            .collect()
    }

    fn widget(text: &str) -> Widget {
        Widget::Text(vec![vec![Span {
            text: text.into(),
            style: StyleToken::Accent,
            ..Span::default()
        }]])
    }

    fn screen(state: &State) -> String {
        let look = state.look(40);
        state
            .transcript
            .iter()
            .flat_map(|c| cell_lines(c, &look))
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn plugin_rendered_cells_replace_the_body() {
        let mut state = fresh(&["tool:read", ASSISTANT]);
        let asked = requests(&finish_both(&mut state));
        assert_eq!(asked.len(), 2, "{asked:?}");
        // Both cells wait in the viewport for their answer.
        assert!(state.take_finished().is_empty());
        for (cell, target) in asked {
            // An escape from the plugin is stripped at the `plugin_ui` boundary.
            let text = format!("\u{1b}[2J{target} by plugin");
            let msg = PluginUiMsg::ItemRendered {
                cell,
                widget: Some(widget(&text)),
            };
            update(&mut state, Msg::Plugin(msg));
        }
        insta::assert_snapshot!("plugin_rendered", screen(&state));
        assert_eq!(state.take_finished().len(), 2);
    }

    #[test]
    fn fallback_after_a_timeout() {
        let mut builtin = fresh(&[]);
        assert!(requests(&finish_both(&mut builtin)).is_empty());
        // The host folded its 20 ms deadline into `None`.
        let mut missed = fresh(&["tool:read", ASSISTANT]);
        for (cell, _) in requests(&finish_both(&mut missed)) {
            let msg = PluginUiMsg::ItemRendered { cell, widget: None };
            update(&mut missed, Msg::Plugin(msg));
        }
        // No answer at all: `WAIT_TICKS` gives the cell back.
        let mut silent = fresh(&["tool:read", ASSISTANT]);
        let asked = requests(&finish_both(&mut silent));
        for _ in 0..WAIT_TICKS {
            assert!(silent.transcript.iter().any(|c| !c.done()));
            update(&mut silent, Msg::Tick);
        }
        insta::assert_snapshot!("fallback_after_timeout", screen(&missed));
        assert_eq!(screen(&missed), screen(&builtin));
        assert_eq!(screen(&silent), screen(&builtin));
        // A late answer never repaints a cell that already fell back.
        for (cell, _) in asked {
            let msg = PluginUiMsg::ItemRendered {
                cell,
                widget: Some(widget("late")),
            };
            update(&mut silent, Msg::Plugin(msg));
        }
        assert_eq!(screen(&silent), screen(&builtin));
    }
}
