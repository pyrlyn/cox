// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The glyph table (T14.1): every symbol the TUI prints that is not plain
//! ASCII, in one place. The terminal owns the font, so what cox owns is a
//! set that degrades to ASCII when the environment cannot show it, and that
//! a user can override glyph by glyph (`[tui.icons]`) — a Nerd Font user
//! substitutes their own. Separate from the render modules so cells, diffs,
//! markdown, the status line and the pickers print one agreed set.

use cox_protocol::config::TuiConfig;
use cox_protocol::types::PermissionMode;
use unicode_width::UnicodeWidthStr;

use cox_sanitize as text;

/// One set. `Copy` because `Look` carries it into every cell render; an
/// override leaks its string, which is what a process-lifetime config value
/// costs and why `resolve` is called once at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyphs {
    /// Prefix of a user message.
    pub user: &'static str,
    /// Marks an attachment under a user message.
    pub attach: &'static str,
    /// Prefix of a tool call header.
    pub tool: &'static str,
    /// Prefix of a thinking cell.
    pub think: &'static str,
    /// A tool call that succeeded.
    pub ok: &'static str,
    /// A tool call that failed, and an error cell.
    pub fail: &'static str,
    /// Prefix of a diff header.
    pub diff: &'static str,
    /// Before the branch name in the status line (T15.2).
    pub branch: &'static str,
    /// The removed-lines sign in `+n −m`.
    pub minus: &'static str,
    /// Before the worktree name in the status line (T27.3).
    pub worktree: &'static str,
    /// Unordered list marker.
    pub bullet: &'static str,
    /// Block quote rail, and (T24.4) a settled tool card's left-edge rail.
    pub quote: &'static str,
    /// Repeated for a horizontal rule and a table's header rule.
    pub rule: &'static str,
    /// Between fields of the status line and the picker rows.
    pub sep: &'static str,
    /// The selected row of a picker.
    pub cursor: &'static str,
    /// Where typing lands in the approval modal's edit field.
    pub caret: &'static str,
    /// Elision, in folded output and in `thinking…`.
    pub ellipsis: &'static str,
    /// Brackets a compaction summary.
    pub dash: &'static str,
    /// The composer prompt per permission mode (T25.2): default, plan,
    /// auto, bypass — the mode is readable where the user is typing.
    pub prompt: &'static str,
    pub plan: &'static str,
    pub auto: &'static str,
    pub bypass: &'static str,
    /// Frames of the running-tool spinner, in order.
    pub spinner: &'static [&'static str],
}

/// The default set: one column per glyph (except `attach`, an emoji), all of
/// it in fonts that ship with a modern terminal.
pub const UNICODE: Glyphs = Glyphs {
    user: "›",
    attach: "📎",
    tool: "⚙",
    think: "∴",
    ok: "✓",
    fail: "✗",
    diff: "±",
    branch: "⎇",
    minus: "−",
    worktree: "⧉",
    bullet: "•",
    quote: "│",
    rule: "─",
    sep: "·",
    cursor: "▸",
    caret: "▏",
    ellipsis: "…",
    dash: "—",
    prompt: ">",
    plan: "▷",
    auto: "»",
    bypass: "!",
    spinner: &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"],
};

/// The fallback: ASCII only, for a terminal whose font or encoding cannot
/// show the set above (a `TERM=dumb`, a non-UTF-8 locale, a bitmap font).
pub const ASCII: Glyphs = Glyphs {
    user: ">",
    attach: "@",
    tool: "*",
    think: ":",
    ok: "+",
    fail: "x",
    diff: "*",
    branch: "#",
    minus: "-",
    worktree: "wt",
    bullet: "-",
    quote: "|",
    rule: "-",
    sep: "|",
    cursor: ">",
    caret: "_",
    ellipsis: "...",
    dash: "-",
    prompt: ">",
    plan: "~",
    auto: ">>",
    bypass: "!",
    spinner: &["|", "/", "-", "\\"],
};

impl Default for Glyphs {
    fn default() -> Self {
        UNICODE
    }
}

/// `tui.glyphs` and `[tui.icons]` resolved into the set the TUI prints.
pub fn resolve(cfg: &TuiConfig) -> Glyphs {
    let mut glyphs = match cfg.glyphs.as_str() {
        "ascii" => ASCII,
        "unicode" => UNICODE,
        // `auto`, and anything unrecognised: ask the environment.
        _ => {
            if utf8(env("TERM").as_deref(), locale().as_deref()) {
                UNICODE
            } else {
                ASCII
            }
        }
    };
    for (name, glyph) in &cfg.icons {
        // Project config is repository input: strip escapes, and take only a
        // glyph narrow enough that it cannot misalign a column. A rejected
        // override leaves the built-in in place rather than failing the run.
        let clean = text::sanitize(glyph);
        if !clean.is_empty() && clean.width() <= 2 {
            glyphs.set(name, String::leak(clean));
        }
    }
    glyphs
}

impl Glyphs {
    /// Replaces one glyph by its `[tui.icons]` key. An unknown key is
    /// ignored rather than fatal — a config typo must not cost a session.
    fn set(&mut self, name: &str, glyph: &'static str) {
        let field = match name {
            "user" => &mut self.user,
            "attach" => &mut self.attach,
            "tool" => &mut self.tool,
            "think" => &mut self.think,
            "ok" => &mut self.ok,
            "fail" => &mut self.fail,
            "diff" => &mut self.diff,
            "branch" => &mut self.branch,
            "minus" => &mut self.minus,
            "worktree" => &mut self.worktree,
            "bullet" => &mut self.bullet,
            "quote" => &mut self.quote,
            "rule" => &mut self.rule,
            "sep" => &mut self.sep,
            "cursor" => &mut self.cursor,
            "caret" => &mut self.caret,
            "ellipsis" => &mut self.ellipsis,
            "dash" => &mut self.dash,
            "prompt" => &mut self.prompt,
            "plan" => &mut self.plan,
            "auto" => &mut self.auto,
            "bypass" => &mut self.bypass,
            _ => return,
        };
        *field = glyph;
    }

    /// The composer prompt for `mode`.
    pub fn mode(&self, mode: PermissionMode) -> &'static str {
        match mode {
            PermissionMode::Default => self.prompt,
            PermissionMode::Plan => self.plan,
            PermissionMode::Auto => self.auto,
            PermissionMode::Bypass => self.bypass,
        }
    }

    /// Frame `tick` of the spinner.
    pub fn spin(&self, tick: u64) -> &'static str {
        let i = usize::try_from(tick % self.spinner.len() as u64).unwrap_or(0);
        self.spinner.get(i).copied().unwrap_or("*")
    }
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// The locale that decides the encoding, in POSIX precedence order.
fn locale() -> Option<String> {
    env("LC_ALL")
        .or_else(|| env("LC_CTYPE"))
        .or_else(|| env("LANG"))
}

/// Whether the terminal can be expected to show non-ASCII: a UTF-8 locale,
/// or none at all (every terminal cox targets defaults to UTF-8; only a
/// locale that names another encoding is evidence against it).
fn utf8(term: Option<&str>, locale: Option<&str>) -> bool {
    if term == Some("dumb") {
        return false;
    }
    match locale {
        Some(l) => {
            let l = l.to_ascii_uppercase();
            l.contains("UTF-8") || l.contains("UTF8")
        }
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn cfg(glyphs: &str, icons: &[(&str, &str)]) -> TuiConfig {
        TuiConfig {
            glyphs: glyphs.to_string(),
            icons: icons
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect::<HashMap<_, _>>(),
            ..TuiConfig::default()
        }
    }

    #[test]
    fn ascii_set_is_ascii_only() {
        let g = ASCII;
        let all: Vec<&str> = vec![
            g.user, g.attach, g.tool, g.think, g.ok, g.fail, g.diff, g.branch, g.minus, g.bullet,
            g.quote, g.rule, g.sep, g.cursor, g.caret, g.ellipsis, g.dash, g.prompt, g.plan,
            g.auto, g.bypass,
        ];
        for s in all.iter().chain(g.spinner) {
            assert!(s.is_ascii(), "{s:?} is not ASCII");
        }
    }

    #[test]
    fn a_non_utf8_locale_falls_back_to_ascii() {
        assert!(utf8(Some("xterm-256color"), Some("en_US.UTF-8")));
        assert!(utf8(Some("xterm-256color"), None));
        assert!(!utf8(Some("xterm-256color"), Some("en_US.ISO-8859-1")));
        assert!(!utf8(Some("dumb"), Some("en_US.UTF-8")));
    }

    #[test]
    fn an_icon_override_replaces_one_glyph_and_keeps_the_rest() {
        let g = resolve(&cfg("unicode", &[("tool", "󰅱"), ("nope", "!")]));
        assert_eq!(g.tool, "󰅱");
        assert_eq!(g.ok, UNICODE.ok);
    }

    #[test]
    fn an_override_is_sanitised_and_a_wide_one_is_refused() {
        let g = resolve(&cfg("ascii", &[("ok", "\u{1b}[31mA"), ("fail", "AAA")]));
        assert_eq!(g.ok, "A");
        assert_eq!(g.fail, ASCII.fail);
    }
}
