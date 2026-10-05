// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox_tui::term::Caps` (T23.0): what the terminal in front of us can
//! actually do. Every later terminal-feature task (T23.1 Kitty keyboard
//! protocol, T23.2 scrolling regions, T23.3 OSC 8/52/9, …) reads a field
//! here instead of re-deriving its own environment guess, so the guess is
//! made once and stays consistent across the TUI.
//!
//! `detect` is a pure environment heuristic (unit-testable without a real
//! terminal). `query` is the one real-I/O refinement: it asks the terminal
//! directly whether it understands the Kitty keyboard protocol, bounded by
//! our own timeout rather than crossterm's fixed 2 s, so a terminal that
//! never answers never delays startup. `apply` lets `[tui.caps]` override
//! any field by name, for a terminal this guesses wrong about.

use std::collections::HashMap;
use std::io::IsTerminal;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

/// How long `query` waits for the terminal's keyboard-protocol reply before
/// giving up and keeping whatever `detect` already guessed. Short enough
/// that an unresponsive terminal (or a pipe that looks like a tty) never
/// delays startup noticeably — the same budget `color::OSC11_TIMEOUT` uses
/// for the background-colour query.
pub const KITTY_QUERY_TIMEOUT: Duration = Duration::from_millis(100);

/// Which terminal features the TUI may use. Every field is a conservative
/// guess (`false` for anything not positively recognised) so an unknown
/// terminal loses features gracefully instead of breaking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Caps {
    /// 24-bit colour (an RGB triple) prints as sent, not just the nearest
    /// ANSI/256 colour.
    pub truecolor: bool,
    /// The Kitty keyboard protocol: distinct press/release/repeat events
    /// and unambiguous key reporting (T23.1).
    pub kitty_keyboard: bool,
    /// OSC 8 hyperlinks.
    pub osc8: bool,
    /// OSC 52 clipboard write.
    pub osc52: bool,
    /// OSC 9 desktop notifications.
    pub osc9: bool,
    /// OSC 9;4 (ConEmu-style) progress reporting.
    pub osc9_4: bool,
    /// Focus-in/out reporting (DECSET 1004).
    pub focus: bool,
    /// An inline image protocol (Kitty graphics or iTerm2's own).
    pub images: bool,
    /// Running inside tmux: some escapes tmux forwards to the outer
    /// terminal, others it swallows (see `detect`'s doc comment).
    pub inside_tmux: bool,
    /// Running over SSH: purely informational today; a later task may use
    /// it to skip a query that would otherwise round-trip the network.
    pub inside_ssh: bool,
}

impl Caps {
    /// Environment-only guess. `env` abstracts `std::env::var` so the table
    /// test below can feed a dozen terminal fingerprints without touching
    /// the process environment.
    ///
    /// Two families: `kitty_family` (Kitty, Ghostty, WezTerm, foot) speaks
    /// both the Kitty keyboard and graphics protocols; the broader `modern`
    /// set adds terminals with their own truecolor + OSC 8/52/9 support
    /// (iTerm2, VS Code, Alacritty, Windows Terminal) but not those two
    /// Kitty-specific protocols. Inside tmux the keyboard protocol is
    /// turned off outright (tmux does not forward the query reliably) but
    /// OSC 8/52 are left alone — tmux does forward those, same fact T22.6
    /// established for OSC 11.
    pub fn detect(env: &dyn Fn(&str) -> Option<String>) -> Caps {
        let colorterm = env("COLORTERM");
        let term = env("TERM").unwrap_or_default();
        let term_program = env("TERM_PROGRAM").unwrap_or_default();
        let no_color = env("NO_COLOR").is_some();
        let inside_tmux = env("TMUX").is_some();
        let inside_ssh = env("SSH_TTY").is_some();

        let kitty_family = env("KITTY_WINDOW_ID").is_some()
            || term == "xterm-kitty"
            || matches!(term_program.as_str(), "ghostty" | "WezTerm")
            || term.starts_with("foot");
        let modern = kitty_family
            || matches!(term_program.as_str(), "iTerm.app" | "vscode")
            || term == "alacritty"
            || env("WT_SESSION").is_some();

        let truecolor = !no_color
            && (modern
                || colorterm
                    .as_deref()
                    .is_some_and(|c| c == "truecolor" || c == "24bit"));
        let osc9_4 = kitty_family || env("WT_SESSION").is_some();
        let images = kitty_family || term_program == "iTerm.app";

        Caps {
            truecolor,
            kitty_keyboard: kitty_family && !inside_tmux,
            osc8: modern,
            osc52: modern,
            osc9: modern,
            osc9_4,
            // DECSET 1004 focus reporting is a near-universal xterm
            // extension; only a genuinely unset `TERM` (piped output, no
            // real terminal in front of us) turns it off.
            focus: !term.is_empty(),
            images,
            inside_tmux,
            inside_ssh,
        }
    }

    /// Asks the terminal directly whether it supports the Kitty keyboard
    /// protocol (`CSI ?u`, backstopped by a primary-device-attributes
    /// query crossterm sends right behind it so a non-supporting terminal's
    /// reply still arrives promptly). Returns whether the query produced a
    /// definitive answer — `false` leaves `kitty_keyboard` at whatever
    /// `detect` already guessed, fail-open. Skipped outright when stdout is
    /// not a tty (piped output, `doctor --json` in CI) since there is
    /// nothing to query.
    pub fn query(&mut self, timeout: Duration) -> bool {
        if !std::io::stdout().is_terminal() {
            return false;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        // crossterm's own internal timeout is a fixed 2 s; running it on a
        // helper thread and bounding the wait with `recv_timeout` lets the
        // caller keep its own, shorter budget. The thread is left to finish
        // on its own if we give up first — harmless, since it only reports
        // into a channel nothing is left listening on.
        std::thread::spawn(move || {
            let _ = tx.send(crossterm::terminal::supports_keyboard_enhancement());
        });
        match rx.recv_timeout(timeout) {
            Ok(Ok(supported)) => {
                self.kitty_keyboard = supported;
                true
            }
            _ => false,
        }
    }

    /// `[tui.caps]`: any named field here overrides the detected/queried
    /// value. An unrecognised key is ignored rather than rejected — a typo
    /// in config should not crash the TUI (same fail-open rule extensions
    /// follow).
    pub fn apply(&mut self, overrides: &HashMap<String, bool>) {
        for (name, value) in overrides {
            match name.as_str() {
                "truecolor" => self.truecolor = *value,
                "kitty_keyboard" => self.kitty_keyboard = *value,
                "osc8" => self.osc8 = *value,
                "osc52" => self.osc52 = *value,
                "osc9" => self.osc9 = *value,
                "osc9_4" => self.osc9_4 = *value,
                "focus" => self.focus = *value,
                "images" => self.images = *value,
                _ => {}
            }
        }
    }

    /// Every field as `(name, value)`, in the order `doctor::check_terminal`
    /// prints them.
    pub fn fields(&self) -> [(&'static str, bool); 10] {
        [
            ("truecolor", self.truecolor),
            ("kitty_keyboard", self.kitty_keyboard),
            ("osc8", self.osc8),
            ("osc52", self.osc52),
            ("osc9", self.osc9),
            ("osc9_4", self.osc9_4),
            ("focus", self.focus),
            ("images", self.images),
            ("inside_tmux", self.inside_tmux),
            ("inside_ssh", self.inside_ssh),
        ]
    }
}

/// Where a turn is, as OSC 9;4 shows it on the tab or taskbar (T23.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Progress {
    #[default]
    Idle,
    /// A turn runs; its length is unknown, so indeterminate.
    Busy,
    /// A turn waits on the user (an approval, an `ask_user` question).
    Paused,
}

/// The OSC 9;4 bytes for `p` in ConEmu/Windows Terminal numbering: 0
/// clears, 3 is indeterminate, 4 is paused. The caller checks
/// `Caps::osc9_4`; a terminal without it may print the bytes.
pub fn progress(p: Progress) -> &'static str {
    match p {
        Progress::Idle => "\x1b]9;4;0;0\x1b\\",
        Progress::Busy => "\x1b]9;4;3;0\x1b\\",
        Progress::Paused => "\x1b]9;4;4;0\x1b\\",
    }
}

/// Whether the terminal is VTE-based (GNOME Terminal, Tilix, …), which
/// shows OSC 777 notifications and ignores OSC 9 (T23.5).
pub fn is_vte(env: &dyn Fn(&str) -> Option<String>) -> bool {
    env("VTE_VERSION").is_some()
}

/// What `notification` writes on this terminal, for `cox doctor`.
pub fn notify_via(caps: &Caps, vte: bool) -> &'static str {
    match (vte, caps.osc9) {
        (true, _) => "OSC 777 + BEL",
        (false, true) => "OSC 9 + BEL",
        (false, false) => "BEL",
    }
}

/// The bytes that ring the terminal (T23.5): an OSC 777 (VTE) or OSC 9
/// desktop notification when the terminal shows one, then a `BEL` so a
/// terminal without either still flags the tab. `title` and `body` can
/// carry tool and model text, so every control character goes — one left
/// in would end the OSC early and turn the rest into terminal input.
pub fn notification(caps: &Caps, vte: bool, title: &str, body: &str) -> String {
    let clean = |s: &str| -> String {
        crate::text::sanitize(s)
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect()
    };
    let osc = match (vte, caps.osc9) {
        (true, _) => format!("\x1b]777;notify;{};{}\x07", clean(title), clean(body)),
        (false, true) => format!("\x1b]9;{}\x07", clean(body)),
        (false, false) => String::new(),
    };
    format!("{osc}\x07")
}

/// OSC 52 clipboard write (T23.4): `Pc = c`, the default clipboard — cox
/// tracks no mouse selection to justify the `p` (primary) destination
/// crossterm's `osc52` feature also offers. Ends with ST (`ESC \`), the
/// terminator crossterm's own `osc!` macro uses despite its doc comment
/// naming BEL. The caller checks `Caps::osc52`; a terminal without it may
/// print the bytes as visible text.
pub fn copy(text: &str) -> String {
    // RFC 4648 standard alphabet with `=` padding, the encoding every
    // terminal expects after `OSC 52`.
    format!("\x1b]52;c;{}\x1b\\", STANDARD.encode(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `vars` is `&[(key, value)]`; anything not listed reads as unset,
    /// same as a real environment that does not export it.
    fn caps_from(vars: &[(&str, &str)]) -> Caps {
        let map: HashMap<&str, &str> = vars.iter().copied().collect();
        Caps::detect(&|key| map.get(key).map(|v| v.to_string()))
    }

    /// (name, env vars, expected `Caps`) — one row of the table test below.
    type Case = (&'static str, &'static [(&'static str, &'static str)], Caps);

    #[test]
    fn caps_table_of_twelve_environments() {
        let cases: &[Case] = &[
            (
                "Ghostty",
                &[
                    ("TERM_PROGRAM", "ghostty"),
                    ("TERM", "xterm-ghostty"),
                    ("COLORTERM", "truecolor"),
                ],
                Caps {
                    truecolor: true,
                    kitty_keyboard: true,
                    osc8: true,
                    osc52: true,
                    osc9: true,
                    osc9_4: true,
                    focus: true,
                    images: true,
                    inside_tmux: false,
                    inside_ssh: false,
                },
            ),
            (
                "Kitty",
                &[
                    ("KITTY_WINDOW_ID", "1"),
                    ("TERM", "xterm-kitty"),
                    ("COLORTERM", "truecolor"),
                ],
                Caps {
                    truecolor: true,
                    kitty_keyboard: true,
                    osc8: true,
                    osc52: true,
                    osc9: true,
                    osc9_4: true,
                    focus: true,
                    images: true,
                    inside_tmux: false,
                    inside_ssh: false,
                },
            ),
            (
                "WezTerm",
                &[
                    ("TERM_PROGRAM", "WezTerm"),
                    ("TERM", "xterm-256color"),
                    ("COLORTERM", "truecolor"),
                ],
                Caps {
                    truecolor: true,
                    kitty_keyboard: true,
                    osc8: true,
                    osc52: true,
                    osc9: true,
                    osc9_4: true,
                    focus: true,
                    images: true,
                    inside_tmux: false,
                    inside_ssh: false,
                },
            ),
            (
                "iTerm2",
                &[
                    ("TERM_PROGRAM", "iTerm.app"),
                    ("TERM", "xterm-256color"),
                    ("COLORTERM", "truecolor"),
                ],
                Caps {
                    truecolor: true,
                    kitty_keyboard: false,
                    osc8: true,
                    osc52: true,
                    osc9: true,
                    osc9_4: false,
                    focus: true,
                    images: true,
                    inside_tmux: false,
                    inside_ssh: false,
                },
            ),
            (
                "Terminal.app",
                &[
                    ("TERM_PROGRAM", "Apple_Terminal"),
                    ("TERM", "xterm-256color"),
                ],
                Caps {
                    truecolor: false,
                    kitty_keyboard: false,
                    osc8: false,
                    osc52: false,
                    osc9: false,
                    osc9_4: false,
                    focus: true,
                    images: false,
                    inside_tmux: false,
                    inside_ssh: false,
                },
            ),
            (
                "Alacritty",
                &[("TERM", "alacritty"), ("COLORTERM", "truecolor")],
                Caps {
                    truecolor: true,
                    kitty_keyboard: false,
                    osc8: true,
                    osc52: true,
                    osc9: true,
                    osc9_4: false,
                    focus: true,
                    images: false,
                    inside_tmux: false,
                    inside_ssh: false,
                },
            ),
            (
                "Windows Terminal",
                &[
                    ("WT_SESSION", "guid"),
                    ("TERM", "xterm-256color"),
                    ("COLORTERM", "truecolor"),
                ],
                Caps {
                    truecolor: true,
                    kitty_keyboard: false,
                    osc8: true,
                    osc52: true,
                    osc9: true,
                    osc9_4: true,
                    focus: true,
                    images: false,
                    inside_tmux: false,
                    inside_ssh: false,
                },
            ),
            (
                "VS Code",
                &[
                    ("TERM_PROGRAM", "vscode"),
                    ("TERM", "xterm-256color"),
                    ("COLORTERM", "truecolor"),
                ],
                Caps {
                    truecolor: true,
                    kitty_keyboard: false,
                    osc8: true,
                    osc52: true,
                    osc9: true,
                    osc9_4: false,
                    focus: true,
                    images: false,
                    inside_tmux: false,
                    inside_ssh: false,
                },
            ),
            (
                "foot",
                &[("TERM", "foot"), ("COLORTERM", "truecolor")],
                Caps {
                    truecolor: true,
                    kitty_keyboard: true,
                    osc8: true,
                    osc52: true,
                    osc9: true,
                    osc9_4: true,
                    focus: true,
                    images: true,
                    inside_tmux: false,
                    inside_ssh: false,
                },
            ),
            (
                "tmux-inside-Ghostty",
                &[
                    ("TMUX", "/tmp/tmux-1000/default,1234,0"),
                    ("TERM_PROGRAM", "ghostty"),
                    ("TERM", "tmux-256color"),
                    ("COLORTERM", "truecolor"),
                ],
                Caps {
                    truecolor: true,
                    // The one field tmux does not forward: the keyboard
                    // protocol query would hang or mislead, so it is off.
                    kitty_keyboard: false,
                    osc8: true,
                    osc52: true,
                    osc9: true,
                    osc9_4: true,
                    focus: true,
                    images: true,
                    inside_tmux: true,
                    inside_ssh: false,
                },
            ),
            (
                "SSH",
                &[("SSH_TTY", "/dev/ttys001"), ("TERM", "xterm-256color")],
                Caps {
                    truecolor: false,
                    kitty_keyboard: false,
                    osc8: false,
                    osc52: false,
                    osc9: false,
                    osc9_4: false,
                    focus: true,
                    images: false,
                    inside_tmux: false,
                    inside_ssh: true,
                },
            ),
            (
                "NO_COLOR",
                &[
                    ("NO_COLOR", "1"),
                    ("TERM_PROGRAM", "ghostty"),
                    ("TERM", "xterm-ghostty"),
                    ("COLORTERM", "truecolor"),
                ],
                Caps {
                    // `NO_COLOR` beats every other colour signal (same rule
                    // `color::from_env` follows) but leaves every other
                    // protocol alone — hyperlinks and the keyboard protocol
                    // are not colour.
                    truecolor: false,
                    kitty_keyboard: true,
                    osc8: true,
                    osc52: true,
                    osc9: true,
                    osc9_4: true,
                    focus: true,
                    images: true,
                    inside_tmux: false,
                    inside_ssh: false,
                },
            ),
        ];
        for (name, vars, expected) in cases.iter().copied() {
            assert_eq!(caps_from(vars), expected, "{name}");
        }
    }

    #[test]
    fn config_overrides_win_over_detection_and_ignore_unknown_keys() {
        let mut caps = caps_from(&[("TERM_PROGRAM", "ghostty"), ("COLORTERM", "truecolor")]);
        assert!(caps.osc8);
        let overrides = HashMap::from([("osc8".to_string(), false), ("made_up".to_string(), true)]);
        caps.apply(&overrides);
        assert!(!caps.osc8, "osc8 = false overrides the detected true");
        assert!(
            caps.truecolor,
            "an untouched field keeps its detected value"
        );
    }

    #[test]
    fn notification_picks_the_sequence_and_strips_controls() {
        let osc9 = Caps {
            osc9: true,
            ..Caps::default()
        };
        let body = "rm -rf x\x07\x1b]9;pwned\x1b\\";
        assert_eq!(
            notification(&osc9, false, "cox", body),
            "\x1b]9;rm -rf x\x07\x07"
        );
        assert_eq!(
            notification(&osc9, true, "cox", "turn done"),
            "\x1b]777;notify;cox;turn done\x07\x07"
        );
        assert_eq!(notification(&Caps::default(), false, "cox", "x"), "\x07");
    }

    #[test]
    fn query_skips_a_non_tty_stdout_without_blocking() {
        // The test harness's stdout is never a tty, so this must return
        // `false` (no change) well within the timeout rather than hang.
        let mut caps = Caps::default();
        let changed = caps.query(Duration::from_millis(50));
        assert!(!changed);
        assert!(!caps.kitty_keyboard);
    }

    /// T23.4, kept through the switch to the `base64` crate (A81): the
    /// payload against known vectors (empty, and one/two/three trailing
    /// bytes, which is where padding differs).
    #[test]
    fn copy_pads_the_payload_like_rfc_4648() {
        for (text, b64) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v")] {
            assert_eq!(copy(text), format!("\x1b]52;c;{b64}\x1b\\"));
        }
    }

    #[test]
    fn copy_writes_osc52_with_the_default_clipboard() {
        assert_eq!(copy("cell text"), "\x1b]52;c;Y2VsbCB0ZXh0\x1b\\");
    }
}
