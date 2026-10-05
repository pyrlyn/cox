// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The TUI's adapter over the plugin UI service (T33.23, PL§8): maps
//! `Cmd::Plugin`'s `PluginRequest` to `cox_session::plugin_ui`'s neutral
//! request and its answer back to `Msg::Plugin`. T52.13 moved the service
//! itself into `cox-session` so the desktop app serves the same hosts with
//! the same deadlines; this module keeps only what is the TUI's own — its
//! cell reference for an item render and its feed.

use std::sync::Arc;
use std::thread::JoinHandle;

use cox_plugin::{PluginHost, Redraw};
use cox_session::plugin_ui::{self as service, ItemSource, PluginAnswer};
use cox_tui::item_render::{CellRef, RenderSource};
use cox_tui::state::{Msg, PluginRequest, PluginUiMsg};
use tokio::sync::mpsc::{Receiver, Sender};

/// Serves `requests` on the service's own thread, answering on `feed`.
pub(crate) fn serve(
    hosts: Vec<(String, Arc<PluginHost>)>,
    requests: Receiver<PluginRequest>,
    feed: Sender<Msg>,
) -> std::io::Result<JoinHandle<()>> {
    service::serve(hosts, requests, feed, split, |cell, answer| {
        Msg::Plugin(join(cell, answer))
    })
}

/// One request's answer, as `serve` gives it (the service's
/// `plugin_ui::answer` under the TUI's types). Only the tests call it:
/// `serve` goes through the service's own loop.
#[cfg(test)]
pub(crate) fn answer(host: Option<&PluginHost>, request: PluginRequest) -> PluginUiMsg {
    let (request, cell) = split(request);
    join(cell, service::answer(host, request))
}

/// The neutral request, and the cell an item render answers for.
fn split(request: PluginRequest) -> (service::PluginRequest, Option<CellRef>) {
    match request {
        PluginRequest::Render { plugin, input } => {
            (service::PluginRequest::Render { plugin, input }, None)
        }
        PluginRequest::Command { plugin, name, args } => {
            (service::PluginRequest::Command { plugin, name, args }, None)
        }
        PluginRequest::Key { plugin, name } => (service::PluginRequest::Key { plugin, name }, None),
        PluginRequest::RenderItem {
            plugin,
            cell,
            target,
            source,
            width,
        } => {
            let source = match source {
                RenderSource::Tool { call, result } => ItemSource::Tool { call, result },
                RenderSource::Assistant { text } => ItemSource::Assistant { text },
            };
            let request = service::PluginRequest::RenderItem {
                plugin,
                target,
                source,
                width,
            };
            (request, Some(cell))
        }
        PluginRequest::Stop { plugin } => (service::PluginRequest::Stop { plugin }, None),
    }
}

/// The TUI's message for an answer.
fn join(cell: Option<CellRef>, answer: PluginAnswer) -> PluginUiMsg {
    match answer {
        PluginAnswer::Rendered {
            plugin,
            slot,
            widget,
        } => PluginUiMsg::Rendered {
            plugin,
            slot,
            widget,
        },
        PluginAnswer::Missed { plugin, slot } => PluginUiMsg::Missed { plugin, slot },
        PluginAnswer::Command { plugin, out } => PluginUiMsg::Command { plugin, out },
        PluginAnswer::ItemRendered { plugin, widget } => match cell {
            Some(cell) => PluginUiMsg::ItemRendered { cell, widget },
            // Only a `RenderItem` answers `ItemRendered`, and `split` kept
            // its cell; a redraw would be harmless if that ever changed.
            None => PluginUiMsg::Redraw { plugin },
        },
        PluginAnswer::Redraw { plugin } => PluginUiMsg::Redraw { plugin },
    }
}

/// The `Redraw` a `PluginTap` calls when a plugin's `Effects.redraw` is set.
/// `try_send`: the tap's pump thread must not wait on the TUI; a redraw lost
/// to a full feed is repainted by the plugin's next one.
pub(crate) fn redraw(feed: Sender<Msg>) -> Redraw {
    service::redraw(move |plugin| {
        let _ = feed.try_send(Msg::Plugin(PluginUiMsg::Redraw { plugin }));
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cox_plugin_api::{Limits, RenderIn, Slot};
    use cox_protocol::plugin::{CommandOut, Widget};
    use std::time::Duration;
    use std::time::Instant;

    // The extism kernel imports a module needs to write its output (P15:
    // the loader takes WAT text).
    const IMPORTS: &str = r#"
      (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
      (import "extism:host/env" "store_u8" (func $store (param i64 i32)))
      (import "extism:host/env" "output_set" (func $output_set (param i64 i64)))"#;
    const WIDGET: &str = r#"{"text":[[{"text":"3 turns"}]]}"#;

    /// A plugin whose `cox_render` is `render`; `cox_init` answers `{}`.
    fn host(render: &str) -> PluginHost {
        export_host("cox_render", render)
    }

    /// A plugin whose `export` is `body`, with `WIDGET` at offset 0.
    fn export_host(export: &str, body: &str) -> PluginHost {
        let json = WIDGET.replace('"', "\\\"");
        let wat = format!(
            r#"(module {IMPORTS}
              (memory 1)
              (data (i32.const 0) "{json}")
              (data (i32.const 64) "{{}}")
              (func $out (param $at i64) (param $n i64) (local $off i64) (local $i i64)
                (local.set $off (call $alloc (local.get $n)))
                (block $done (loop $copy
                  (br_if $done (i64.ge_u (local.get $i) (local.get $n)))
                  (call $store (i64.add (local.get $off) (local.get $i))
                    (i32.load8_u (i32.wrap_i64 (i64.add (local.get $at) (local.get $i)))))
                  (local.set $i (i64.add (local.get $i) (i64.const 1)))
                  (br $copy)))
                (call $output_set (local.get $off) (local.get $n)))
              (func (export "cox_init") (result i32)
                (call $out (i64.const 64) (i64.const 2)) (i32.const 0))
              (func (export "{export}") (result i32) {body}))"#,
        );
        // A long call cap, so only the render deadline can cut a spin short.
        let limits = Limits {
            call_ms: Some(30_000),
            ..Limits::default()
        };
        PluginHost::load("acme", wat.as_bytes(), &limits).expect("module loads")
    }

    fn request() -> PluginRequest {
        PluginRequest::Render {
            plugin: "acme".into(),
            input: RenderIn {
                slot: Slot::StatusLeft,
                width: 24,
                height: 1,
            },
        }
    }

    #[test]
    fn render_answer_carries_the_widget() {
        let body = format!(
            "(call $out (i64.const 0) (i64.const {})) (i32.const 0)",
            WIDGET.len()
        );
        let msg = answer(Some(&host(&body)), request());
        let widget: Widget = serde_json::from_str(WIDGET).expect("widget json");
        assert_eq!(
            msg,
            PluginUiMsg::Rendered {
                plugin: "acme".into(),
                slot: Slot::StatusLeft,
                widget,
            }
        );
    }

    #[test]
    fn slow_render_is_missed_at_its_deadline() {
        let spin = host("(loop $l (br $l)) (i32.const 0)");
        let started = Instant::now();
        let msg = answer(Some(&spin), request());
        assert!(matches!(msg, PluginUiMsg::Missed { .. }), "{msg:?}");
        // `call_ms` is 30 s; only the 20 ms render deadline ends it this soon.
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        assert!(matches!(
            answer(None, request()),
            PluginUiMsg::Missed { .. }
        ));
    }

    /// T33.26: `cox_render_item` answers the cell it was asked for, and a
    /// spin past `RENDER_DEADLINE` answers `None` (the built-in look).
    #[test]
    fn render_item_answers_its_cell_or_none_at_the_deadline() {
        use cox_protocol::ids::ItemId;
        use cox_tui::item_render::CellRef;

        let cell = CellRef::Item(ItemId::new());
        let request = || PluginRequest::RenderItem {
            plugin: "acme".into(),
            cell,
            target: cox_tui::item_render::ASSISTANT.into(),
            source: RenderSource::Assistant { text: "hi".into() },
            width: 40,
        };
        let body = format!(
            "(call $out (i64.const 0) (i64.const {})) (i32.const 0)",
            WIDGET.len()
        );
        let widget: Widget = serde_json::from_str(WIDGET).expect("widget json");
        assert_eq!(
            answer(Some(&export_host("cox_render_item", &body)), request()),
            PluginUiMsg::ItemRendered {
                cell,
                widget: Some(widget),
            }
        );
        let spin = export_host("cox_render_item", "(loop $l (br $l)) (i32.const 0)");
        let started = Instant::now();
        let missed = answer(Some(&spin), request());
        assert_eq!(missed, PluginUiMsg::ItemRendered { cell, widget: None });
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    const COMMAND_JSON: &str = r#"{"kind":"prompt","text":"go"}"#;

    /// A plugin whose `export` (`cox_command` or `cox_key`) is `body`;
    /// `cox_init` answers `{}`, like `host` above.
    fn command_host(export: &str, body: &str) -> PluginHost {
        let json = COMMAND_JSON.replace('"', "\\\"");
        let wat = format!(
            r#"(module {IMPORTS}
              (memory 1)
              (data (i32.const 0) "{json}")
              (data (i32.const 64) "{{}}")
              (func $out (param $at i64) (param $n i64) (local $off i64) (local $i i64)
                (local.set $off (call $alloc (local.get $n)))
                (block $done (loop $copy
                  (br_if $done (i64.ge_u (local.get $i) (local.get $n)))
                  (call $store (i64.add (local.get $off) (local.get $i))
                    (i32.load8_u (i32.wrap_i64 (i64.add (local.get $at) (local.get $i)))))
                  (local.set $i (i64.add (local.get $i) (i64.const 1)))
                  (br $copy)))
                (call $output_set (local.get $off) (local.get $n)))
              (func (export "cox_init") (result i32)
                (call $out (i64.const 64) (i64.const 2)) (i32.const 0))
              (func (export "{export}") (result i32) {body}))"#,
        );
        let limits = Limits {
            call_ms: Some(30_000),
            ..Limits::default()
        };
        PluginHost::load("acme", wat.as_bytes(), &limits).expect("module loads")
    }

    /// T33.25: a `/<id>:<name>` command calls `cox_command` and its
    /// `CommandOut` reaches the TUI as `PluginUiMsg::Command`.
    #[test]
    fn command_answer_carries_command_out() {
        let body = format!(
            "(call $out (i64.const 0) (i64.const {})) (i32.const 0)",
            COMMAND_JSON.len()
        );
        let host = command_host("cox_command", &body);
        let request = PluginRequest::Command {
            plugin: "acme".into(),
            name: "go".into(),
            args: String::new(),
        };
        assert_eq!(
            answer(Some(&host), request),
            PluginUiMsg::Command {
                plugin: "acme".into(),
                out: Some(CommandOut::Prompt { text: "go".into() }),
            }
        );
    }

    /// T33.25: `<leader> <key>` calls `cox_key`, never `cox_command` — a
    /// plugin with only the latter fails open (`out: None`) for a `Key`
    /// request, the same as a missing export always has.
    #[test]
    fn key_request_calls_cox_key_not_cox_command() {
        let body = format!(
            "(call $out (i64.const 0) (i64.const {})) (i32.const 0)",
            COMMAND_JSON.len()
        );
        let host = command_host("cox_command", &body);
        let request = PluginRequest::Key {
            plugin: "acme".into(),
            name: "reset".into(),
        };
        assert_eq!(
            answer(Some(&host), request),
            PluginUiMsg::Command {
                plugin: "acme".into(),
                out: None,
            }
        );
    }
}
