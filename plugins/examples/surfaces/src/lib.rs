//! `cox-plugin-surfaces`: the PL§15 example, written against
//! `cox-plugin-sdk`. One plugin declares every kind of part and lets the host
//! keep the ones the session's surface has (PL§15.6):
//!
//! - shared: a status segment and `/glance:glance`;
//! - terminal only: the leader key `g`;
//! - desktop only: an inspector tab, a toolbar item, a palette action, an
//!   actionable notification and a canvas of two frames whose parts a click
//!   hands to the composer (T33.47.8), so the pane has a plugin to show. It
//!   counts streamed text chunks and redraws every [`REDRAW_EVERY`]th: a
//!   command cannot redraw a canvas, and falsifier 3 of the canvas design
//!   needs one redrawing while a reply streams.
//!
//! The plugin declares all of it unconditionally. Branching on the surface in
//! `cox_init` would work too (`surface_of`), but then the host's filter, the
//! thing this example exists to show, would never run.
//!
//! The declarations and widgets are pure functions so `just plugin-test`
//! checks them on the host target; the exports, which need extism's memory
//! and the host imports, build only for `wasm32` (as in
//! `plugins/examples/rust`). `crates/cox-plugin-fixtures` builds this crate
//! into the package `cox-plugin`'s `surfaces_example` test loads.

use cox_plugin_sdk::ui::Span;
use cox_plugin_sdk::{
    ActionDecl, CanvasAttach, CanvasDecl, CanvasOut, CanvasSelect, CommandDecl, DesktopNotice,
    DesktopOut, Frame, InitOut, KeyDecl, NoticeAction, NoticeLevel, Slot, StyleToken, TabDecl,
    Widget,
};

/// The one command every contribution runs.
pub const COMMAND: &str = "glance";

/// The attribute a click on the canvas reports.
pub const SELECT_ATTR: &str = "data-glance";

/// The event the canvas counts.
pub const CHUNK_EVENT: &str = "text_delta";

/// Streamed chunks between two canvas redraws.
pub const REDRAW_EVERY: u64 = 16;

/// Whether counting from `before` to `after` chunks passed a redraw.
pub fn redraws(before: u64, after: u64) -> bool {
    before / REDRAW_EVERY != after / REDRAW_EVERY
}

/// What `cox_init` declares: every part of every surface.
pub fn declarations() -> InitOut {
    let action = ActionDecl {
        command: COMMAND.into(),
        title: "Glance".into(),
        symbol: "eye".into(),
    };
    InitOut {
        commands: vec![CommandDecl {
            name: COMMAND.into(),
            description: "Count a glance and show it".into(),
        }],
        keys: vec![KeyDecl {
            key: "g".into(),
            name: COMMAND.into(),
            description: "Count a glance".into(),
        }],
        status: vec![Slot::StatusRight, Slot::DesktopInspector],
        subscribe: vec![CHUNK_EVENT.into()],
        desktop: DesktopOut {
            inspector: Some(TabDecl {
                title: "Glance".into(),
                symbol: "eye".into(),
            }),
            toolbar: vec![action.clone()],
            palette: vec![action],
            canvas: Some(CanvasDecl {
                title: "Glance".into(),
                symbol: "eye".into(),
                select_attr: SELECT_ATTR.into(),
            }),
        },
        ..InitOut::default()
    }
}

/// The canvas (`cox_canvas`): the counts, which change as the session goes
/// on, and a legend, which never does, so a redraw reloads only the first
/// frame. Inline styles only, because the frame's policy refuses everything
/// else.
pub fn canvas(glances: u64, chunks: u64) -> CanvasOut {
    let page = |body: String| {
        format!(
            "<!doctype html><html><head><meta name=\"color-scheme\" content=\"light dark\">\
             <style>body{{font:15px -apple-system,sans-serif;margin:16px}}\
             [{SELECT_ATTR}]{{padding:12px;border-radius:8px;margin:8px 0;\
             background:rgba(127,127,127,.15)}}</style></head><body>{body}</body></html>"
        )
    };
    CanvasOut {
        frames: vec![
            Frame {
                id: "count".into(),
                title: "Count".into(),
                width_pt: 320,
                html: page(format!(
                    "<h1 {SELECT_ATTR}=\"count\">{glances} glances</h1>\
                     <p {SELECT_ATTR}=\"chunks\">{chunks} chunks streamed</p>\
                     <p {SELECT_ATTR}=\"command\">Run /glance:glance to count one more.</p>"
                )),
            },
            Frame {
                id: "legend".into(),
                title: "Legend".into(),
                width_pt: 280,
                html: page(format!(
                    "<div {SELECT_ATTR}=\"status\">Status segment: glances N</div>\
                     <div {SELECT_ATTR}=\"inspector\">Inspector tab: the count and a link</div>"
                )),
            },
        ],
    }
}

/// What a click on the canvas adds to the composer (`cox_canvas_select`):
/// a note naming the part, or nothing for a value this plugin never drew.
pub fn select(s: &CanvasSelect, glances: u64, chunks: u64) -> Option<CanvasAttach> {
    let text = match (s.frame.as_str(), s.value.as_str()) {
        ("count", "count") => format!("Glance's count reads {glances} glances."),
        ("count", "chunks") => format!("Glance has seen {chunks} streamed chunks."),
        ("count", "command") => format!("Glance's /glance:{COMMAND} adds one to the count."),
        ("legend", "status") => "Glance's status segment shows the count.".to_string(),
        ("legend", "inspector") => "Glance's inspector tab shows the count and a link.".into(),
        _ => return None,
    };
    Some(CanvasAttach {
        name: format!("Glance: {}", s.value),
        text,
    })
}

/// The notification the desktop shows; its button runs [`COMMAND`] again.
pub fn notice(glances: u64) -> DesktopNotice {
    DesktopNotice {
        level: NoticeLevel::Info,
        title: "Glance".into(),
        body: format!("{glances} so far"),
        actions: vec![NoticeAction {
            label: "Again".into(),
            command: COMMAND.into(),
        }],
    }
}

fn span(text: String, style: StyleToken) -> Span {
    Span {
        text,
        style,
        ..Span::default()
    }
}

/// The status segment (`status.right`).
pub fn status(glances: u64) -> Widget {
    Widget::Text(vec![vec![span(
        format!("glances {glances}"),
        StyleToken::Dim,
    )]])
}

/// The inspector tab's tree (`desktop.inspector`): a count and a link, which
/// the app opens only on the person's click.
pub fn inspector(glances: u64) -> Widget {
    Widget::KeyValue(vec![
        (
            span("glances".into(), StyleToken::Dim),
            vec![span(glances.to_string(), StyleToken::Text)],
        ),
        (
            span("docs".into(), StyleToken::Dim),
            vec![Span {
                link: Some("https://github.com/pyrlyn/cox".into()),
                ..span("cox on GitHub".into(), StyleToken::Accent)
            }],
        ),
    ])
}

/// What `cox_render` draws into `slot`, or nothing for a slot this plugin
/// does not fill.
pub fn draw(slot: Slot, glances: u64) -> Widget {
    match slot {
        Slot::DesktopInspector => inspector(glances),
        Slot::StatusLeft | Slot::StatusRight => status(glances),
        Slot::Panel | Slot::Overlay => Widget::Text(Vec::new()),
    }
}

#[cfg(target_arch = "wasm32")]
mod guest {
    use core::sync::atomic::{AtomicU64, Ordering};

    use cox_plugin_sdk::{
        AbiError, CanvasAttach, CanvasIn, CanvasOut, CanvasSelect, CommandIn, CommandOut, Effects,
        EventBatch, InitIn, InitOut, NoticeLevel, PluginNotice, RenderIn, SdkError, Widget,
        desktop_notify, redraw,
    };

    // In memory only: `cox_render` may not touch kv, and the count is not
    // worth a capability.
    static GLANCES: AtomicU64 = AtomicU64::new(0);
    static CHUNKS: AtomicU64 = AtomicU64::new(0);

    fn counts() -> (u64, u64) {
        (
            GLANCES.load(Ordering::Relaxed),
            CHUNKS.load(Ordering::Relaxed),
        )
    }

    fn init(_: InitIn) -> Result<InitOut, SdkError> {
        Ok(super::declarations())
    }

    fn glance() -> Result<CommandOut, SdkError> {
        let n = GLANCES.fetch_add(1, Ordering::Relaxed) + 1;
        // A refused redraw only delays the new count until the next one.
        let _ = redraw();
        match desktop_notify(&super::notice(n)) {
            // The terminal has no actionable notifications (PL§15.6): say it
            // in the transcript instead.
            Err(SdkError::Host(AbiError::NotOnThisSurface { .. })) => {
                Ok(CommandOut::Notice(PluginNotice {
                    level: NoticeLevel::Info,
                    text: format!("{n} glances"),
                }))
            }
            // On the desktop the tab opens even when the notification was
            // refused (rate limit, a full queue): it is what the person asked
            // for.
            _ => Ok(CommandOut::OpenInspector),
        }
    }

    fn command(c: CommandIn) -> Result<CommandOut, SdkError> {
        if c.name == super::COMMAND {
            glance()
        } else {
            Ok(CommandOut::Nothing)
        }
    }

    fn render(r: RenderIn) -> Result<Widget, SdkError> {
        Ok(super::draw(r.slot, GLANCES.load(Ordering::Relaxed)))
    }

    // Dropped chunks count too: the ring dropped them, they still streamed.
    fn on_event(batch: EventBatch) -> Result<Effects, SdkError> {
        let n = batch.events.len() as u64 + batch.dropped;
        let before = CHUNKS.fetch_add(n, Ordering::Relaxed);
        Ok(Effects {
            redraw: super::redraws(before, before + n),
            ..Effects::default()
        })
    }

    fn canvas(_: CanvasIn) -> Result<CanvasOut, SdkError> {
        let (glances, chunks) = counts();
        Ok(super::canvas(glances, chunks))
    }

    fn canvas_select(s: CanvasSelect) -> Result<Option<CanvasAttach>, SdkError> {
        let (glances, chunks) = counts();
        Ok(super::select(&s, glances, chunks))
    }

    // `key` is the terminal's leader entry and takes the same input.
    cox_plugin_sdk::register!(
        init => init,
        command => command,
        key => command,
        render => render,
        on_event => on_event,
        canvas => canvas,
        canvas_select => canvas_select,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declares_a_part_for_every_surface() {
        let out = declarations();
        assert_eq!(out.commands[0].name, COMMAND);
        assert_eq!(out.keys[0].key, "g");
        assert_eq!(out.status, [Slot::StatusRight, Slot::DesktopInspector]);
        assert!(out.desktop.inspector.is_some());
        assert_eq!(out.desktop.toolbar.len(), 1);
        assert_eq!(out.desktop.palette.len(), 1);
        assert_eq!(out.subscribe, [CHUNK_EVENT]);
        let canvas = out.desktop.canvas.expect("a canvas");
        assert_eq!(canvas.check(), Ok(()));
        assert_eq!(canvas.select_attr, SELECT_ATTR);
    }

    #[test]
    fn canvas_draws_two_frames_within_the_caps() {
        let (kept, dropped) = canvas(4, 0).capped();
        assert!(dropped.is_empty(), "{dropped:?}");
        let ids: Vec<_> = kept.frames.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["count", "legend"]);
        assert!(kept.frames[0].html.contains("4 glances"));
    }

    #[test]
    fn a_count_changes_only_the_count_frame() {
        let before = canvas(1, 0);
        for after in [canvas(2, 0), canvas(1, 16)] {
            assert_ne!(before.frames[0].html, after.frames[0].html);
            assert_eq!(before.frames[1].html, after.frames[1].html);
        }
    }

    #[test]
    fn the_canvas_redraws_once_per_sixteen_chunks() {
        assert!(!redraws(0, 15));
        assert!(redraws(15, 16));
        assert!(!redraws(16, 31));
        assert!(redraws(10, 40), "a batch over two marks still redraws");
        assert!(!redraws(5, 5), "an empty batch does not");
    }

    #[test]
    fn every_drawn_part_gives_an_attachment_within_the_caps() {
        for frame in canvas(3, 7).frames {
            let marker = format!("{SELECT_ATTR}=\"");
            for part in frame.html.split(&marker).skip(1) {
                let value = part.split('"').next().unwrap_or_default();
                let click = CanvasSelect {
                    frame: frame.id.clone(),
                    value: value.into(),
                };
                assert_eq!(click.check(), Ok(()));
                let attach = select(&click, 3, 7).expect("a drawn part attaches");
                assert!(attach.clone().capped().is_ok(), "{attach:?}");
            }
        }
    }

    #[test]
    fn a_value_glance_never_drew_attaches_nothing() {
        let click = |frame: &str, value: &str| CanvasSelect {
            frame: frame.into(),
            value: value.into(),
        };
        assert_eq!(select(&click("count", "nope"), 1, 0), None);
        assert_eq!(select(&click("legend", "count"), 1, 0), None);
    }

    #[test]
    fn every_action_names_a_declared_command_and_a_valid_symbol() {
        let out = declarations();
        let names: Vec<_> = out.commands.iter().map(|c| c.name.as_str()).collect();
        let actions = out.desktop.toolbar.iter().chain(&out.desktop.palette);
        for a in actions {
            assert!(names.contains(&a.command.as_str()), "{}", a.command);
            assert!(cox_plugin_sdk::is_symbol_name(&a.symbol), "{}", a.symbol);
        }
        let tab = out.desktop.inspector.expect("a tab");
        assert!(cox_plugin_sdk::is_symbol_name(&tab.symbol));
    }

    #[test]
    fn notification_button_runs_a_declared_command() {
        let n = notice(3);
        assert!(n.body.starts_with('3'));
        assert!(n.actions.len() <= cox_plugin_sdk::MAX_NOTICE_ACTIONS);
        assert_eq!(n.actions[0].command, COMMAND);
    }

    #[test]
    fn widgets_stay_within_the_host_limits() {
        for slot in [Slot::StatusRight, Slot::DesktopInspector] {
            assert!(draw(slot, 7).within_limits());
        }
    }

    #[test]
    fn inspector_links_to_an_https_url() {
        let Widget::KeyValue(rows) = inspector(1) else {
            panic!("a key-value widget");
        };
        let link = rows[1].1[0].link.as_deref();
        assert!(link.is_some_and(|l| l.starts_with("https://")));
    }
}
