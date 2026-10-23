// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The end-to-end check of PL§15 (T33.45.8): the built `plugins/examples/
//! surfaces` package loads in a terminal and a desktop session, each keeps
//! the parts its surface has, and the other surface's parts are dropped with
//! one notice or answered with `NotOnThisSurface`. Its canvas exports run
//! from the compiled guest too (T33.47.8), which the inline-WAT hosts of
//! `live.rs` cannot return frames from. Its own module, apart from
//! `live.rs`'s inline-WAT tests, because it needs the compiled guest and the
//! SDK path it exercises, not a hand-written module.

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use cox_plugin_api::{
        CanvasAttach, CanvasIn, CanvasOut, CanvasSelect, CommandIn, CommandOut, NoticeLevel,
        PluginNotice, RenderIn, Slot, Surface, Widget,
    };
    use cox_plugin_fixtures::{SURFACES_DIR, SURFACES_WASM};
    use cox_protocol::PluginStore;
    use cox_protocol::config::PluginsConfig;
    use cox_protocol::ids::SessionId;
    use cox_protocol::types::Level;

    use crate::discover::load_manifest;
    use crate::hostfn::tests::MemKv;
    use crate::{Lane, LivePlugins};

    const WAIT: Duration = Duration::from_secs(10);

    /// The example loaded the way a granted plugin is: its real manifest, so
    /// its real grant lines, under `surface`.
    fn started(surface: Surface) -> LivePlugins {
        let dir = Path::new(SURFACES_DIR);
        let (manifest, _) =
            load_manifest(dir, &dir.join("plugin.toml"), None).expect("the manifest validates");
        let store: Arc<dyn PluginStore> = Arc::new(MemKv::default());
        let mut live = LivePlugins::default();
        live.set_surface(surface);
        live.load(&manifest, SURFACES_WASM, store).expect("loads");
        let warnings = live.start(&PluginsConfig::default(), SessionId::new(), Path::new("/w"));
        assert!(warnings.is_empty(), "{warnings:?}");
        live
    }

    fn call<O: serde::de::DeserializeOwned>(
        live: &LivePlugins,
        export: &str,
        input: &impl serde::Serialize,
    ) -> O {
        live.hosts()[0]
            .1
            .call(Lane::Control, export, input, WAIT)
            .expect("the call runs")
            .expect("the export exists")
    }

    fn command(name: &str) -> CommandIn {
        CommandIn {
            name: name.into(),
            args: String::new(),
        }
    }

    fn render(live: &LivePlugins, slot: Slot) -> Widget {
        call(
            live,
            "cox_render",
            &RenderIn {
                slot,
                width: 40,
                height: 10,
            },
        )
    }

    fn notice_text(live: &LivePlugins) -> String {
        let notices = live.take_notices();
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert_eq!(notices[0].0, Level::Info);
        notices.into_iter().next().map(|n| n.1).unwrap_or_default()
    }

    #[test]
    fn terminal_session_keeps_the_key_and_drops_the_desktop_parts() {
        let live = started(Surface::Terminal);
        let plugin = &live.plugins()[0];
        let init = plugin.init_out().expect("init ran");

        assert_eq!(plugin.granted_commands()[0].name, "glance");
        assert_eq!(plugin.granted_keys()[0].key, "g");
        assert_eq!(plugin.granted_status(), [Slot::StatusRight]);
        assert!(init.desktop.inspector.is_none());
        assert!(init.desktop.toolbar.is_empty() && init.desktop.palette.is_empty());
        assert!(init.desktop.canvas.is_none());

        let notice = notice_text(&live);
        let parts = [
            "desktop.inspector",
            "desktop.toolbar",
            "desktop.palette",
            "desktop.canvas",
        ];
        for part in parts {
            assert!(notice.contains(part), "{notice}");
        }
        assert!(!notice.contains("keys"), "{notice}");
    }

    #[test]
    fn desktop_session_keeps_the_desktop_parts_and_drops_the_key() {
        let live = started(Surface::Desktop);
        let plugin = &live.plugins()[0];
        let init = plugin.init_out().expect("init ran");

        assert_eq!(plugin.granted_commands()[0].name, "glance");
        assert!(plugin.granted_keys().is_empty());
        assert_eq!(
            plugin.granted_status(),
            [Slot::StatusRight, Slot::DesktopInspector]
        );
        assert_eq!(
            init.desktop.inspector.as_ref().expect("a tab").title,
            "Glance"
        );
        assert_eq!(init.desktop.toolbar.len(), 1);
        assert_eq!(init.desktop.palette.len(), 1);
        assert_eq!(
            init.desktop.canvas.as_ref().expect("a canvas").select_attr,
            "data-glance"
        );

        let notice = notice_text(&live);
        assert!(notice.contains("keys"), "{notice}");
        assert!(
            notice.ends_with("keys"),
            "only the key is dropped: {notice}"
        );
    }

    #[test]
    fn the_command_falls_back_to_a_notice_on_the_terminal() {
        let live = started(Surface::Terminal);
        // The host answered `cox_desktop_notify` with `NotOnThisSurface`, so
        // the plugin said it in the transcript instead.
        let out: CommandOut = call(&live, "cox_command", &command("glance"));
        assert_eq!(
            out,
            CommandOut::Notice(PluginNotice {
                level: NoticeLevel::Info,
                text: "1 glances".into()
            })
        );
        // The leader key runs the same handler and the count carries over.
        let out: CommandOut = call(&live, "cox_key", &command("glance"));
        assert!(matches!(out, CommandOut::Notice(n) if n.text == "2 glances"));
    }

    #[test]
    fn the_command_opens_the_inspector_on_the_desktop() {
        let live = started(Surface::Desktop);
        let out: CommandOut = call(&live, "cox_command", &command("glance"));
        assert_eq!(out, CommandOut::OpenInspector);
    }

    #[test]
    fn each_surface_renders_its_slots_with_the_live_count() {
        let live = started(Surface::Desktop);
        let _: CommandOut = call(&live, "cox_command", &command("glance"));

        let Widget::Text(lines) = render(&live, Slot::StatusRight) else {
            panic!("a text widget");
        };
        assert_eq!(lines[0][0].text, "glances 1");

        let Widget::KeyValue(rows) = render(&live, Slot::DesktopInspector) else {
            panic!("a key-value widget");
        };
        assert_eq!(rows[0].1[0].text, "1");
        assert_eq!(
            rows[1].1[0].link.as_deref(),
            Some("https://github.com/pyrlyn/cox")
        );
    }

    #[test]
    fn the_canvas_draws_two_frames_and_a_click_becomes_an_attachment() {
        let live = started(Surface::Desktop);
        let _: CommandOut = call(&live, "cox_command", &command("glance"));
        let size = CanvasIn {
            width_pt: 900,
            height_pt: 600,
        };
        let out: CanvasOut = call(&live, "cox_canvas", &size);
        let (kept, dropped) = out.capped();
        assert!(dropped.is_empty(), "{dropped:?}");
        let ids: Vec<_> = kept.frames.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["count", "parts"]);
        assert!(kept.frames[0].html.contains("<h1>1</h1>"), "the live count");

        let click = CanvasSelect {
            frame: "parts".into(),
            value: "toolbar".into(),
        };
        assert_eq!(click.check(), Ok(()));
        let attach: Option<CanvasAttach> = call(&live, "cox_canvas_select", &click);
        let attach = attach
            .expect("an attachment")
            .capped()
            .expect("within caps");
        assert_eq!(attach.name, "Glance: toolbar");
        assert!(attach.text.contains("\"parts\""), "{}", attach.text);
    }
}
