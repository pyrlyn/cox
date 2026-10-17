// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Hostile strings (T5.6): fifty things a model, a tool or a file could print
//! to escape its cell — every one renders inside a frame with the lines
//! around it intact, and `sanitize` leaves no control character behind.

use cox_protocol::ids::ItemId;
use cox_protocol::types::{Event, ItemKind, PermissionMode, SandboxMode};
use cox_tui::state::{Msg, State, update};
use cox_tui::text::{sanitize, sanitize_with};
use cox_tui::view::{buffer_to_string, render};

const ESC: &str = "\u{1b}";

fn hostile() -> Vec<(&'static str, String)> {
    let long_word = "x".repeat(400);
    let long_words = "word ".repeat(80);
    let wide = "漢".repeat(200);
    let combining = format!("a{}", "\u{301}".repeat(300));
    vec![
        ("osc52 clipboard", format!("{ESC}]52;c;aGVsbG8=\u{7}")),
        ("osc0 title", format!("{ESC}]0;pwned\u{7}")),
        ("osc2 title st", format!("{ESC}]2;pwned{ESC}\\")),
        (
            "osc8 hyperlink",
            format!("{ESC}]8;;http://evil\u{7}click{ESC}]8;;\u{7}"),
        ),
        ("osc7 cwd", format!("{ESC}]7;file://h/tmp\u{7}")),
        ("osc9 notify", format!("{ESC}]9;hi\u{7}")),
        ("osc133 shell", format!("{ESC}]133;A\u{7}")),
        ("osc1337 iterm", format!("{ESC}]1337;File=:AAAA\u{7}")),
        ("osc10 colour", format!("{ESC}]10;#000000\u{7}")),
        ("osc unterminated", format!("{ESC}]0;never closed")),
        ("cursor up", format!("{ESC}[5A")),
        ("cursor home", format!("{ESC}[H")),
        ("cursor position", format!("{ESC}[1;1H")),
        ("clear screen", format!("{ESC}[2J")),
        ("clear scrollback", format!("{ESC}[3J")),
        ("erase line", format!("{ESC}[2K")),
        ("sgr red", format!("{ESC}[31m")),
        ("sgr reset", format!("{ESC}[0m")),
        ("sgr 256", format!("{ESC}[38;5;196m")),
        ("sgr rgb", format!("{ESC}[38;2;1;2;3m")),
        ("alt screen", format!("{ESC}[?1049h")),
        ("hide cursor", format!("{ESC}[?25l")),
        ("mouse on", format!("{ESC}[?1000h")),
        ("scroll region", format!("{ESC}[1;5r")),
        ("insert lines", format!("{ESC}[10L")),
        ("csi unterminated", format!("{ESC}[12;34")),
        ("csi private", format!("{ESC}[>0c")),
        ("csi intermediate", format!("{ESC}[ q")),
        ("ris", format!("{ESC}c")),
        ("save cursor", format!("{ESC}7")),
        ("restore cursor", format!("{ESC}8")),
        ("charset g0", format!("{ESC}(0")),
        ("decaln", format!("{ESC}#8")),
        ("dcs sixel", format!("{ESC}Pq#0;2;0;0;0#0~~{ESC}\\")),
        ("dcs request", format!("{ESC}P$qm{ESC}\\")),
        ("apc", format!("{ESC}_Gf=24;AAAA{ESC}\\")),
        ("pm", format!("{ESC}^secret{ESC}\\")),
        ("sos", format!("{ESC}Xsecret{ESC}\\")),
        ("c1 csi", "\u{9b}2J".to_string()),
        ("c1 osc", "\u{9d}0;t\u{7}".to_string()),
        ("bel", "\u{7}".to_string()),
        ("backspace overwrite", "abc\u{8}\u{8}\u{8}xyz".to_string()),
        ("carriage return overwrite", "harmless\rEVIL".to_string()),
        ("vt ff nul", "\u{b}\u{c}\u{0}".to_string()),
        ("so si del", "\u{e}\u{f}\u{7f}".to_string()),
        ("rtl override", "\u{202e}gnp.exe".to_string()),
        ("isolates", "\u{2066}\u{2067}\u{2068}\u{2069}".to_string()),
        ("embeddings", "\u{202a}\u{202b}\u{202c}\u{202d}".to_string()),
        (
            "zero width run",
            "\u{200b}\u{200b}\u{200b}\u{feff}\u{2060}".to_string(),
        ),
        ("zwj run", "\u{200d}\u{200d}\u{200d}".to_string()),
        ("long word", long_word),
        ("long words", long_words),
        ("wide", wide),
        ("combining flood", combining),
        (
            "osc in fence",
            format!("```sh\n{ESC}]52;c;AAAA\u{7}echo\n```"),
        ),
        ("esc inside osc", format!("{ESC}]0;{ESC}[2J\u{7}")),
    ]
}

fn unsafe_char(c: char) -> bool {
    c.is_control() && c != '\n' && c != '\t'
        || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{80}'..='\u{9f}')
}

/// §1.15 invariant 14.
#[test]
fn sanitize_strips_escapes() {
    let cases = hostile();
    assert!(cases.len() >= 50);
    for (name, s) in &cases {
        // An unterminated OSC/CSI may eat the rest of its line, as a real
        // terminal would; it must never reach the next one.
        let clean = sanitize(&format!("{s}\ntail"));
        assert!(!clean.chars().any(unsafe_char), "{name}: {clean:?}");
        assert!(clean.ends_with("\ntail"), "{name}: {clean:?}");
        assert!(!clean.contains("\u{200b}"), "{name}");
    }
    assert!(sanitize_with(&format!("{ESC}[2J"), true).contains('␛'));
    assert_eq!(sanitize_with("\u{7}", true), "␇");
    assert_eq!(sanitize_with("\u{202e}", true), "⇄");
}

/// Every hostile string, as an assistant reply, stays inside its cell: the
/// frame has no control characters and the text after it is still there.
#[test]
fn sanitize_hostile_strings_render_inside_the_cell() {
    for (name, s) in hostile() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::ReadOnly);
        let item = ItemId::new();
        update(
            &mut state,
            Msg::Event(Event::ItemStarted {
                item,
                kind: ItemKind::AssistantMessage {
                    text: format!("before\n\n{s}\n\nafter"),
                },
            }),
        );
        // Tall enough that even the 400-column cases fit without scrolling.
        let frame = buffer_to_string(&render(&state, 40, 40));
        assert!(!frame.chars().any(unsafe_char), "{name}: {frame:?}");
        assert!(frame.contains("after"), "{name}: {frame}");
        assert!(frame.contains("before"), "{name}: {frame}");
    }
}

#[test]
fn sanitize_frame_shows_markers_when_verbose() {
    let mut state = State::new(PermissionMode::Default, SandboxMode::ReadOnly);
    state.marks = true;
    let text = format!(
        "title {ESC}]0;x\u{7} clip {ESC}]52;c;AAAA\u{7} up {ESC}[5A bell \u{7} rtl \u{202e}exe zw a\u{200b}\u{200b}b"
    );
    update(
        &mut state,
        Msg::Event(Event::ItemStarted {
            item: ItemId::new(),
            kind: ItemKind::AssistantMessage { text },
        }),
    );
    insta::assert_snapshot!(buffer_to_string(&render(&state, 60, 5)));
}
