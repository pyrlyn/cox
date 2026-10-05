// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `sanitize` (T5.6, moved to its own crate by T32.1): the one place a
//! string the model, a tool, an MCP server or a file wrote is made safe to
//! print. Escape sequences (ESC/CSI/OSC/DCS and their C1 forms), C0 controls
//! other than `\n`/`\t`, bidi overrides and isolates, and zero-width
//! characters in suspicious runs are removed — or, with markers on (`-v`),
//! replaced by a visible glyph — so nothing can move the cursor, set the
//! title, write the clipboard or reorder what the user reads. Every cell
//! renderer calls this at the boundary; a tool result is the one place cox
//! shows a whole file someone else wrote.
//!
//! Separate from `cox-tui` (guard (b) and reuse (d), `docs/design/crates.md`)
//! so the headless surface (`crates/cox/src/plain.rs`) and anything else
//! that only needs the guard do not pull in the whole TUI. `cox-tui`
//! re-exports this crate at its old `text` path, so `cox_tui::text::sanitize`
//! keeps working for existing callers.
//!
//! [`redact`] is the sibling output guard for secrets rather than escapes
//! (T28.4, moved here by T33.9).

// why: this crate was not under `missing_docs` before the workspace lints;
// drop this allow once its public items are documented.
#![allow(missing_docs)]

pub mod redact;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Strips everything unsafe; see the module doc.
pub fn sanitize(s: &str) -> String {
    sanitize_with(s, false)
}

/// `marks`: leave a glyph where each removal happened (`␛` for an escape
/// sequence, the control picture for a C0 byte, `⇄` for a bidi control,
/// `∅` for a zero-width run) so a user with `-v` sees that something was cut.
pub fn sanitize_with(s: &str, marks: bool) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\u{1b}' => {
                i = skip_escape(&chars, i + 1);
                if marks {
                    out.push('␛');
                }
                continue;
            }
            // C1 controls: the 8-bit forms of CSI/OSC/DCS/SOS/PM/APC start a
            // sequence; the rest are single bytes.
            '\u{9b}' => {
                i = skip_csi(&chars, i + 1);
                if marks {
                    out.push('␛');
                }
                continue;
            }
            '\u{9d}' | '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => {
                i = skip_string(&chars, i + 1);
                if marks {
                    out.push('␛');
                }
                continue;
            }
            '\u{80}'..='\u{9f}' => {
                if marks {
                    out.push('␛');
                }
            }
            '\n' | '\t' => out.push(c),
            '\u{0}'..='\u{1f}' => {
                if marks {
                    out.push(char::from_u32(0x2400 + u32::from(c)).unwrap_or('␀'));
                }
            }
            '\u{7f}' => {
                if marks {
                    out.push('␡');
                }
            }
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => {
                if marks {
                    out.push('⇄');
                }
            }
            _ if is_zero_width(c) => {
                // A lone ZWJ/ZWNJ between two visible characters is script
                // shaping (emoji sequences, Persian); anything else is a
                // run that hides or pads text.
                let joiner = matches!(c, '\u{200c}' | '\u{200d}');
                let prev_visible = out.chars().last().is_some_and(|p| !p.is_whitespace());
                let next_visible = chars
                    .get(i + 1)
                    .is_some_and(|n| !n.is_whitespace() && !is_zero_width(*n));
                if joiner && prev_visible && next_visible {
                    out.push(c);
                } else {
                    while chars.get(i + 1).is_some_and(|n| is_zero_width(*n)) {
                        i += 1;
                    }
                    if marks {
                        out.push('∅');
                    }
                }
            }
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

fn is_zero_width(c: char) -> bool {
    matches!(c, '\u{200b}'..='\u{200d}' | '\u{2060}' | '\u{feff}')
}

/// After an ESC: returns the index just past the sequence it introduces.
fn skip_escape(chars: &[char], i: usize) -> usize {
    match chars.get(i) {
        Some('[') => skip_csi(chars, i + 1),
        Some(']' | 'P' | 'X' | '^' | '_') => skip_string(chars, i + 1),
        // Two-character escapes (`ESC c`, `ESC 7`, `ESC ( 0` takes one more).
        Some('(' | ')' | '*' | '+' | '#' | '%') => (i + 2).min(chars.len()),
        Some(_) => i + 1,
        None => i,
    }
}

/// CSI: parameter and intermediate bytes up to a final byte in 0x40–0x7E.
/// A newline ends an unterminated sequence so it cannot eat the next line.
fn skip_csi(chars: &[char], mut i: usize) -> usize {
    while let Some(&c) = chars.get(i) {
        if c == '\n' {
            return i;
        }
        i += 1;
        if ('\u{40}'..='\u{7e}').contains(&c) {
            return i;
        }
    }
    i
}

/// OSC/DCS/SOS/PM/APC: up to BEL or ST (`ESC \` or U+009C); a newline ends
/// an unterminated one for the same reason as above.
fn skip_string(chars: &[char], mut i: usize) -> usize {
    while let Some(&c) = chars.get(i) {
        match c {
            '\n' => return i,
            '\u{7}' | '\u{9c}' => return i + 1,
            '\u{1b}' if chars.get(i + 1) == Some(&'\\') => return i + 2,
            _ => i += 1,
        }
    }
    i
}

/// `s` cut to at most `width` columns by display width, `ellipsis` (the
/// glyph table's, so an ASCII terminal gets `...`) marking the cut.
pub fn truncate(s: &str, width: usize, ellipsis: &str) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let room = width.saturating_sub(ellipsis.width());
    let mut acc = 0;
    let mut out = String::new();
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if acc + w > room {
            break;
        }
        acc += w;
        out.push(c);
    }
    out.push_str(ellipsis);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_newlines_tabs_and_shaping_joiners() {
        assert_eq!(sanitize("a\tb\nc"), "a\tb\nc");
        assert_eq!(sanitize("👩\u{200d}💻"), "👩\u{200d}💻");
        assert_eq!(sanitize("a\u{200b}\u{200b}b"), "ab");
        assert_eq!(sanitize_with("a\u{200b}\u{200b}b", true), "a∅b");
    }

    #[test]
    fn sanitize_cuts_an_unterminated_osc_at_the_line_end() {
        assert_eq!(sanitize("\u{1b}]0;title\nnext"), "\nnext");
    }

    #[test]
    fn sanitize_strips_model_osc8() {
        let link = "\u{1b}]8;;https://evil.test\u{1b}\\click\u{1b}]8;;\u{1b}\\ me";
        assert_eq!(sanitize(link), "click me");
        assert_eq!(sanitize("\u{9d}8;;file:///etc\u{9c}x"), "x");
    }

    #[test]
    fn truncate_counts_columns_not_bytes() {
        assert_eq!(truncate("漢字漢字", 5, "…"), "漢字…");
        assert_eq!(truncate("short", 10, "…"), "short");
        assert_eq!(truncate("abcdefgh", 6, "..."), "abc...");
    }
}
