//! `cox-plugin-surfaces`: the PL§15 example, written against
//! `cox-plugin-sdk`. One plugin declares every kind of part and lets the host
//! keep the ones the session's surface has (PL§15.6):
//!
//! - shared: a status segment and `/glance:glance`;
//! - terminal only: the leader key `g`;
//! - desktop only: an inspector tab, a toolbar item, a palette action, an
//!   actionable notification and a canvas of two pages whose parts a click
//!   hands to the composer (`docs/design/plugin-canvas.md`).
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

/// The attribute the canvas pages mark their selectable parts with.
pub const SELECT_ATTR: &str = "data-glance";

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

/// The style both pages share. `prefers-color-scheme` because the pane
/// follows the window's appearance and passes no field for it (design §3).
const PAGE_STYLE: &str = "<style>\
body{font:14px -apple-system,sans-serif;margin:16px;color:#1d1d1f;background:#fff}\
@media (prefers-color-scheme:dark){body{color:#f5f5f7;background:#1d1d1f}}\
[data-glance]{padding:8px 12px;margin:8px 0;border:1px solid #8884;border-radius:8px}\
h1{font-size:28px;margin:0}\
</style>";

fn page(body: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">{PAGE_STYLE}</head><body>{body}</body></html>"
    )
}

/// What `cox_canvas` draws: the count and the parts a person can point at,
/// side by side as two frames of different widths. Static HTML only: the
/// app runs no page script, so the pages need none.
pub fn frames(glances: u64) -> CanvasOut {
    let count = page(&format!(
        "<div data-glance=\"count\"><h1>{glances}</h1>glances so far</div>\
         <div data-glance=\"hint\">Run /glance:glance to count one more.</div>"
    ));
    let parts = page(
        "<div data-glance=\"status\">Status segment: <b>status.right</b></div>\
         <div data-glance=\"inspector\">Inspector tab: <b>Glance</b></div>\
         <div data-glance=\"toolbar\">Toolbar item and palette action</div>\
         <div data-glance=\"notification\">Notification with an <b>Again</b> button</div>",
    );
    CanvasOut {
        frames: vec![
            Frame {
                id: "count".into(),
                title: "Count".into(),
                width_pt: 320,
                html: count,
            },
            Frame {
                id: "parts".into(),
                title: "Desktop parts".into(),
                width_pt: 480,
                html: parts,
            },
        ],
    }
}

/// What a click on a canvas part adds to the composer, or nothing for a
/// frame this plugin did not draw. The value is the page's own attribute,
/// already checked by the host, so it is quoted rather than trusted.
pub fn attach(select: &CanvasSelect) -> Option<CanvasAttach> {
    if !frames(0).frames.iter().any(|f| f.id == select.frame) {
        return None;
    }
    Some(CanvasAttach {
        name: format!("Glance: {}", select.value),
        text: format!(
            "The person pointed at the {:?} part of the Glance canvas's {:?} page.",
            select.value, select.frame
        ),
    })
}

#[cfg(target_arch = "wasm32")]
mod guest {
    use core::sync::atomic::{AtomicU64, Ordering};

    use cox_plugin_sdk::{
        AbiError, CanvasAttach, CanvasIn, CanvasOut, CanvasSelect, CommandIn, CommandOut, InitIn,
        InitOut, NoticeLevel, PluginNotice, RenderIn, SdkError, Widget, desktop_notify, redraw,
    };

    // In memory only: `cox_render` may not touch kv, and the count is not
    // worth a capability.
    static GLANCES: AtomicU64 = AtomicU64::new(0);

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

    fn canvas(_: CanvasIn) -> Result<CanvasOut, SdkError> {
        Ok(super::frames(GLANCES.load(Ordering::Relaxed)))
    }

    fn canvas_select(s: CanvasSelect) -> Result<Option<CanvasAttach>, SdkError> {
        Ok(super::attach(&s))
    }

    // `key` is the terminal's leader entry and takes the same input.
    cox_plugin_sdk::register!(
        init => init,
        command => command,
        key => command,
        render => render,
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
        let canvas = out.desktop.canvas.expect("a canvas");
        assert_eq!(canvas.check(), Ok(()));
        assert_eq!(canvas.select_attr, SELECT_ATTR);
    }

    #[test]
    fn canvas_draws_two_frames_the_host_keeps_whole() {
        let out = frames(7);
        let (kept, dropped) = out.clone().capped();
        assert!(dropped.is_empty(), "{dropped:?}");
        assert_eq!(kept, out, "nothing cut or clamped");
        assert_eq!(kept.frames.len(), 2);
        assert!(kept.frames[0].html.contains("<h1>7</h1>"));
        for frame in &kept.frames {
            assert!(frame.html.contains(SELECT_ATTR), "{}", frame.id);
            assert!(!frame.html.contains("<script"), "{}", frame.id);
        }
    }

    #[test]
    fn a_click_on_a_drawn_frame_becomes_a_capped_attachment() {
        let click = |frame: &str| CanvasSelect {
            frame: frame.into(),
            value: "count".into(),
        };
        let attach = attach(&click("count")).expect("an attachment");
        assert_eq!(attach.name, "Glance: count");
        assert_eq!(attach.clone().capped(), Ok(attach));
        assert_eq!(super::attach(&click("other")), None);
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
