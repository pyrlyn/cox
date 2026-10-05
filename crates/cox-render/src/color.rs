// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Colour depth (T14.2): what the terminal in front of us can actually
//! show. syntect hands the markdown renderer 24-bit colours and the cells
//! use named ones; a terminal that understands neither would print them as
//! noise or ignore `NO_COLOR` entirely. Every colour is therefore mapped
//! once, on the finished buffer — the single place both the live screen
//! (`view`) and the scrollback (`app`'s `insert_before`) pass through — so
//! no render site has to know what the terminal supports.
//!
//! Also owns background detection for `tui.theme = "auto"` (T22.6):
//! `detect_dark` queries OSC 11 once, before raw mode, and hands the caller
//! a fail-open `Option<bool>` — the same "ask the environment, never block
//! or guess wrong" shape `resolve` already uses for colour depth.

use std::time::Duration;

use cox_protocol::config::TuiConfig;
use ratatui::buffer::Buffer;
use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Depth {
    /// `NO_COLOR`: modifiers only, every colour reset to the terminal's own.
    None,
    /// The eight ANSI colours and their bright forms.
    Ansi16,
    /// The xterm 256-colour cube.
    Ansi256,
    /// 24-bit colour, printed as syntect produced it.
    #[default]
    True,
}

/// `tui.color = auto | none | 16 | 256 | true`; `auto` asks the environment.
pub fn resolve(cfg: &TuiConfig) -> Depth {
    match cfg.color.as_str() {
        "none" => Depth::None,
        "16" => Depth::Ansi16,
        "256" => Depth::Ansi256,
        "true" => Depth::True,
        _ => from_env(
            var("NO_COLOR").as_deref(),
            var("COLORTERM").as_deref(),
            var("TERM").as_deref(),
        ),
    }
}

fn var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// The conservative reading: claim 24-bit only when the terminal says so,
/// because guessing high prints garbage where guessing low prints a nearby
/// colour. `NO_COLOR` (any non-empty value, per the convention) wins.
fn from_env(no_color: Option<&str>, colorterm: Option<&str>, term: Option<&str>) -> Depth {
    if no_color.is_some() || term == Some("dumb") {
        return Depth::None;
    }
    if colorterm.is_some_and(|c| c.contains("truecolor") || c.contains("24bit")) {
        return Depth::True;
    }
    match term {
        Some(t) if t.contains("256") || t.contains("direct") => Depth::Ansi256,
        Some(_) => Depth::Ansi16,
        // No `TERM` at all: the 256-colour cube is safe on anything that can
        // draw the TUI in the first place.
        None => Depth::Ansi256,
    }
}

impl Depth {
    /// `c` as this terminal can show it.
    pub fn map(self, c: Color) -> Color {
        match self {
            Depth::True => c,
            Depth::None => Color::Reset,
            Depth::Ansi256 => match c {
                Color::Rgb(r, g, b) => Color::Indexed(cube(r, g, b)),
                other => other,
            },
            Depth::Ansi16 => match c {
                Color::Rgb(r, g, b) => nearest16(r, g, b),
                other => other,
            },
        }
    }
}

/// Rewrites every colour in `buf` for `depth`, leaving modifiers alone —
/// `NO_COLOR` asks for no colour, not for no bold.
pub fn map_buffer(buf: &mut Buffer, depth: Depth) {
    if depth == Depth::True {
        return;
    }
    let area = buf.area;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            cell.fg = depth.map(cell.fg);
            cell.bg = depth.map(cell.bg);
        }
    }
}

/// `tui.theme = "auto"` (T22.6): how long to wait for the terminal's OSC 11
/// reply before giving up. Short enough that a terminal which never answers
/// (no support, or a pipe with no tty) never delays startup noticeably.
pub const OSC11_TIMEOUT: Duration = Duration::from_millis(100);

/// Below this normalised luma the background reads as dark.
const DARK_LUMA: f32 = 0.5;

/// `tui.theme = "auto"`: query the terminal's background colour over OSC 11
/// and classify it as dark or light. `None` means "could not tell" — tmux
/// does not forward OSC 11 reliably to the outer terminal so it is skipped
/// outright, and any query error (unsupported terminal, no reply within
/// `timeout`, not a tty) is treated the same way. The caller falls back to
/// dark, the same default `tui.theme` had before this query existed.
pub fn detect_dark(timeout: Duration) -> Option<bool> {
    if std::env::var("TMUX").is_ok() {
        return None;
    }
    // `QueryOptions` is `#[non_exhaustive]`: build the default, then set the
    // one field this crate cares about, rather than a struct literal.
    let mut options = terminal_colorsaurus::QueryOptions::default();
    options.timeout = timeout;
    let bg = terminal_colorsaurus::background_color(options).ok()?;
    let (r, g, b) = bg.scale_to_8bit();
    Some(is_dark(r, g, b))
}

/// ITU-R BT.601 luma of an 8-bit RGB colour, normalised to `0.0..=1.0`,
/// compared against the perceptual midpoint.
fn is_dark(r: u8, g: u8, b: u8) -> bool {
    let luma =
        (0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b)) / f32::from(u8::MAX);
    luma <= DARK_LUMA
}

/// The xterm index for an RGB triple: the 24-step grey ramp for a colour
/// whose channels agree, the 6×6×6 cube otherwise.
fn cube(r: u8, g: u8, b: u8) -> u8 {
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    if max - min < 8 {
        // 232..=255 are grey; 8, 18, 28 … 238 are their levels.
        let level = u16::from(max);
        return match level {
            0..=7 => 16,
            238.. => 231,
            _ => 232 + u8::try_from((level - 8) / 10).unwrap_or(23).min(23),
        };
    }
    let step = |v: u8| u8::try_from((u16::from(v) * 5 + 127) / 255).unwrap_or(5);
    16 + 36 * step(r) + 6 * step(g) + step(b)
}

/// The nearest of the sixteen ANSI colours: which channels carry most of the
/// colour decides the hue, the brightest channel decides the bright form.
fn nearest16(r: u8, g: u8, b: u8) -> Color {
    let max = r.max(g).max(b);
    if max < 64 {
        return Color::Black;
    }
    let on = |v: u8| u16::from(v) * 2 >= u16::from(max);
    let bright = max >= 192;
    match (on(r), on(g), on(b)) {
        (true, true, true) if bright => Color::White,
        (true, true, true) => Color::Gray,
        (true, false, false) if bright => Color::LightRed,
        (true, false, false) => Color::Red,
        (false, true, false) if bright => Color::LightGreen,
        (false, true, false) => Color::Green,
        (false, false, true) if bright => Color::LightBlue,
        (false, false, true) => Color::Blue,
        (true, true, false) if bright => Color::LightYellow,
        (true, true, false) => Color::Yellow,
        (true, false, true) if bright => Color::LightMagenta,
        (true, false, true) => Color::Magenta,
        (false, true, true) if bright => Color::LightCyan,
        (false, true, true) => Color::Cyan,
        (false, false, false) => Color::DarkGray,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_color_beats_every_other_signal() {
        assert_eq!(
            from_env(Some("1"), Some("truecolor"), Some("xterm-256color")),
            Depth::None
        );
        assert_eq!(from_env(None, None, Some("dumb")), Depth::None);
    }

    #[test]
    fn truecolor_is_claimed_only_when_the_terminal_says_so() {
        assert_eq!(
            from_env(None, Some("truecolor"), Some("xterm-256color")),
            Depth::True
        );
        assert_eq!(from_env(None, None, Some("xterm-256color")), Depth::Ansi256);
        assert_eq!(from_env(None, None, Some("vt100")), Depth::Ansi16);
    }

    #[test]
    fn rgb_maps_into_the_cube_and_onto_a_named_colour() {
        assert_eq!(
            Depth::Ansi256.map(Color::Rgb(255, 0, 0)),
            Color::Indexed(196)
        );
        assert_eq!(Depth::Ansi256.map(Color::Rgb(0, 0, 0)), Color::Indexed(16));
        assert_eq!(Depth::Ansi256.map(Color::Cyan), Color::Cyan);
        assert_eq!(Depth::Ansi16.map(Color::Rgb(200, 30, 30)), Color::LightRed);
        assert_eq!(Depth::Ansi16.map(Color::Rgb(120, 20, 20)), Color::Red);
        assert_eq!(Depth::Ansi16.map(Color::Rgb(20, 20, 20)), Color::Black);
        assert_eq!(Depth::None.map(Color::Red), Color::Reset);
    }

    #[test]
    fn a_grey_becomes_a_grey_not_a_cube_corner() {
        // 128,128,128 is on the grey ramp, not in the 6×6×6 cube.
        assert!(matches!(cube(128, 128, 128), 232..=255));
    }

    #[test]
    fn luminance_threshold_maps_known_backgrounds() {
        // (terminal/theme default background, expected `is_dark`).
        let cases: &[(&str, (u8, u8, u8), bool)] = &[
            ("xterm default (black)", (0x00, 0x00, 0x00), true),
            ("Solarized Dark", (0x00, 0x2b, 0x36), true),
            ("Solarized Light", (0xfd, 0xf6, 0xe3), false),
            ("Dracula", (0x28, 0x2a, 0x36), true),
            ("Gruvbox Dark", (0x28, 0x28, 0x28), true),
            ("Gruvbox Light", (0xfb, 0xf1, 0xc7), false),
            ("One Dark", (0x28, 0x2c, 0x34), true),
            ("Terminal.app Basic (white)", (0xff, 0xff, 0xff), false),
        ];
        for (name, (r, g, b), expected_dark) in cases.iter().copied() {
            assert_eq!(is_dark(r, g, b), expected_dark, "{name}");
        }
    }

    #[test]
    // why: env::set_var/remove_var are unsafe in edition 2024 (per-process tests).
    #[allow(unsafe_code)]
    fn tmux_skips_the_query_without_touching_the_terminal() {
        // SAFETY: test-only env mutation, no other test in this module reads TMUX.
        unsafe { std::env::set_var("TMUX", "/tmp/tmux-1000/default,1234,0") };
        let result = detect_dark(Duration::from_millis(1));
        unsafe { std::env::remove_var("TMUX") };
        assert_eq!(result, None);
    }
}
