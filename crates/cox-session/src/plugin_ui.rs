// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The plugin UI service (T33.23–T33.26, PL§8; moved here by T52.13, DT§4.7
//! G9): renders, commands, keys and item renders asked of the session's live
//! plugin hosts, each under its deadline, answered as neutral values. Here,
//! not in `crates/cox`, so the TUI and the desktop app share one
//! implementation: each surface keeps only the mapping from its own messages
//! to [`PluginRequest`] and from [`PluginAnswer`] back.

use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cox_plugin::{Lane, Redraw};
use cox_protocol::plugin::{CommandIn, CommandOut, RenderIn, RenderItemIn, Slot, Widget};
use cox_protocol::types::{ItemKind, ToolCall, ToolResult};
use tokio::sync::mpsc::{Receiver, Sender};

// What a surface's `ServeUi` is handed and what its tests load, named here so
// the desktop app reaches them without depending on the extism host (T52.14).
pub use cox_plugin::{LivePlugins, PluginHost};

/// A render's budget (PL§8, PL§12 "render 20 ms").
pub const RENDER_DEADLINE: Duration = Duration::from_millis(20);
/// A command or a key's budget (T33.25): interactive, not render-critical,
/// so it gets the same outer cap `cox_init` and a plugin hook already use.
pub const COMMAND_DEADLINE: Duration = Duration::from_secs(5);
const RENDER: &str = "cox_render";
const COMMAND: &str = "cox_command";
const KEY: &str = "cox_key";
const RENDER_ITEM: &str = "cox_render_item";

/// What a surface asks of one plugin.
#[derive(Debug, Clone, PartialEq)]
pub enum PluginRequest {
    /// Call `plugin`'s `cox_render` with `input`.
    Render { plugin: String, input: RenderIn },
    /// `/<id>:<name>` (T33.25, PL§8): call `plugin`'s `cox_command`.
    Command {
        plugin: String,
        name: String,
        args: String,
    },
    /// `<leader> <key>` (T33.25, PL§8): call `plugin`'s `cox_key`.
    Key { plugin: String, name: String },
    /// A finished item (T33.26): call `plugin`'s `cox_render_item`.
    RenderItem {
        plugin: String,
        target: String,
        source: ItemSource,
        width: u16,
    },
    /// `/plugin remove <id>` confirmed (T33.33, PL§1c): flag `plugin`'s
    /// host stopped. Answered with a plain redraw.
    Stop { plugin: String },
}

impl PluginRequest {
    /// The plugin the request is for.
    pub fn plugin(&self) -> &str {
        match self {
            Self::Render { plugin, .. }
            | Self::Command { plugin, .. }
            | Self::Key { plugin, .. }
            | Self::RenderItem { plugin, .. }
            | Self::Stop { plugin } => plugin,
        }
    }
}

/// What `cox_render_item` renders: a tool call with its result, or an
/// assistant message.
#[derive(Debug, Clone, PartialEq)]
pub enum ItemSource {
    Tool {
        call: Box<ToolCall>,
        result: ToolResult,
    },
    Assistant {
        text: String,
    },
}

/// One request's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum PluginAnswer {
    /// A `cox_render` answer that came back inside its deadline.
    Rendered {
        plugin: String,
        slot: Slot,
        widget: Widget,
    },
    /// A `cox_render` that timed out, failed or does not exist; the surface
    /// keeps the last good render and counts the miss.
    Missed { plugin: String, slot: Slot },
    /// A `cox_command`/`cox_key` answer; `None` is a timeout, an error or a
    /// missing export (PL§4 fails open).
    Command {
        plugin: String,
        out: Option<CommandOut>,
    },
    /// `plugin`'s `cox_render_item` answer; `None` keeps the built-in look.
    ItemRendered {
        plugin: String,
        widget: Option<Widget>,
    },
    /// Render this plugin's slots again (a `Stop`'s answer).
    Redraw { plugin: String },
}

/// Serves `requests` on a thread of its own, since a host call blocks for
/// up to its deadline. Requests that piled up behind a slow render are
/// served once each. `split` takes a surface's request apart into the
/// neutral one and whatever the surface needs back with its answer (the
/// TUI's cell); `join` builds the surface's message. Ends when the surface
/// drops its sender or the feed closes.
pub fn serve<R, K, M>(
    hosts: Vec<(String, Arc<PluginHost>)>,
    mut requests: Receiver<R>,
    feed: Sender<M>,
    split: impl Fn(R) -> (PluginRequest, K) + Send + 'static,
    join: impl Fn(K, PluginAnswer) -> M + Send + 'static,
) -> std::io::Result<JoinHandle<()>>
where
    R: PartialEq + Send + 'static,
    K: Send + 'static,
    M: Send + 'static,
{
    thread::Builder::new()
        .name("cox-plugin-ui".into())
        .spawn(move || {
            while let Some(first) = requests.blocking_recv() {
                let mut batch = vec![first];
                while let Ok(next) = requests.try_recv() {
                    if !batch.contains(&next) {
                        batch.push(next);
                    }
                }
                for request in batch {
                    let (request, keep) = split(request);
                    let host = hosts.iter().find(|(id, _)| id == request.plugin());
                    let answer = answer(host.map(|(_, h)| h.as_ref()), request);
                    if feed.blocking_send(join(keep, answer)).is_err() {
                        return;
                    }
                }
            }
        })
}

/// One request's answer. A render inside `RENDER_DEADLINE` is `Rendered`;
/// a timeout, an error, a missing export or an unknown plugin is `Missed`.
/// A command or a key answers `Command`, `out: None` covering the same
/// misses.
pub fn answer(host: Option<&PluginHost>, request: PluginRequest) -> PluginAnswer {
    match request {
        PluginRequest::Render { plugin, input } => {
            let slot = input.slot;
            let out =
                host.map(|h| h.call::<_, Widget>(Lane::Control, RENDER, &input, RENDER_DEADLINE));
            match out {
                Some(Ok(Some(widget))) => PluginAnswer::Rendered {
                    plugin,
                    slot,
                    widget,
                },
                _ => PluginAnswer::Missed { plugin, slot },
            }
        }
        PluginRequest::Command { plugin, name, args } => {
            let input = CommandIn { name, args };
            PluginAnswer::Command {
                plugin,
                out: command_call(host, COMMAND, &input),
            }
        }
        PluginRequest::RenderItem {
            plugin,
            target,
            source,
            width,
        } => {
            let input = render_item_input(target, source, width);
            let widget = host
                .map(|h| {
                    h.call::<_, Option<Widget>>(Lane::Control, RENDER_ITEM, &input, RENDER_DEADLINE)
                })
                .and_then(|out| out.ok().flatten().flatten());
            PluginAnswer::ItemRendered { plugin, widget }
        }
        PluginRequest::Key { plugin, name } => {
            let input = CommandIn {
                name,
                args: String::new(),
            };
            PluginAnswer::Command {
                plugin,
                out: command_call(host, KEY, &input),
            }
        }
        // T33.33, PL§1c: `WasmTool::is_stopped` is the only reader of this
        // flag. `Stop` has no reply of its own, so it answers with a plain
        // redraw, which re-asks only what is already on screen.
        PluginRequest::Stop { plugin } => {
            if let Some(h) = host {
                h.stop();
            }
            PluginAnswer::Redraw { plugin }
        }
    }
}

/// PL§4's `RenderItemIn`: the call and its result as the model's history
/// carries them, or the assistant item as `ItemKind` serializes it.
fn render_item_input(target: String, source: ItemSource, width: u16) -> RenderItemIn {
    let (call, result) = match source {
        ItemSource::Tool { call, result } => (
            serde_json::to_value(&call).ok(),
            serde_json::to_value(&result),
        ),
        ItemSource::Assistant { text } => (
            None,
            serde_json::to_value(ItemKind::AssistantMessage { text }),
        ),
    };
    RenderItemIn {
        target,
        call,
        result: result.unwrap_or_default(),
        width,
    }
}

/// `cox_command`/`cox_key`'s call, folding a timeout, an error, a missing
/// export or an unknown plugin into one `None` (PL§4 fails open).
fn command_call(host: Option<&PluginHost>, export: &str, input: &CommandIn) -> Option<CommandOut> {
    host?
        .call::<_, CommandOut>(Lane::Control, export, input, COMMAND_DEADLINE)
        .ok()
        .flatten()
}

/// The `Redraw` a `PluginTap` calls when a plugin's `Effects.redraw` is set;
/// `notify` must not block, since the tap's pump thread calls it.
pub fn redraw(notify: impl Fn(String) + Send + Sync + 'static) -> Redraw {
    Arc::new(move |plugin: &str| notify(plugin.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An unknown plugin fails open: a render is missed, a command answers
    /// `None`, an item render keeps the built-in look.
    #[test]
    fn a_request_for_an_unknown_plugin_fails_open() {
        let render = PluginRequest::Render {
            plugin: "acme".into(),
            input: RenderIn {
                slot: Slot::StatusLeft,
                width: 24,
                height: 1,
            },
        };
        assert_eq!(
            answer(None, render),
            PluginAnswer::Missed {
                plugin: "acme".into(),
                slot: Slot::StatusLeft,
            }
        );
        let key = PluginRequest::Key {
            plugin: "acme".into(),
            name: "reset".into(),
        };
        assert_eq!(
            answer(None, key),
            PluginAnswer::Command {
                plugin: "acme".into(),
                out: None,
            }
        );
        let item = PluginRequest::RenderItem {
            plugin: "acme".into(),
            target: "assistant".into(),
            source: ItemSource::Assistant { text: "hi".into() },
            width: 40,
        };
        assert_eq!(
            answer(None, item),
            PluginAnswer::ItemRendered {
                plugin: "acme".into(),
                widget: None,
            }
        );
    }

    /// PL§4: an assistant item reaches `cox_render_item` as `ItemKind`
    /// serializes it, with no call.
    #[test]
    fn an_assistant_item_renders_as_its_item_kind() {
        let input = render_item_input(
            "assistant".into(),
            ItemSource::Assistant { text: "hi".into() },
            40,
        );
        assert_eq!(input.call, None);
        assert_eq!(
            input.result,
            serde_json::to_value(ItemKind::AssistantMessage { text: "hi".into() })
                .expect("item json")
        );
    }
}
