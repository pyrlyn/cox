// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Pure rendering for the terminal: themes and colour tokens, colour-depth
//! mapping, markdown with syntax highlighting, diffs, SVG export, glyph sets
//! and OSC 8 link marking. Split out of `cox-tui` (T32.2, `docs/design/crates.md`
//! C2) because it alone pulls in syntect, two-face, pulldown-cmark and
//! terminal-colorsaurus, so an edit to the TUI's state machine does not
//! recompile them. No state, no event loop: text and settings in, ratatui
//! spans and buffers out. `cox-tui` re-exports every module at its old path.
//! Markdown and highlighting first produce a UI-neutral `doc::StyledDoc`
//! (T37.7); everything that speaks ratatui sits behind the default `ratatui`
//! feature, so the desktop app can depend on this crate without it.

// why: this crate was not under `missing_docs` before the workspace lints;
// drop this allow once its public items are documented.
#![allow(missing_docs)]

#[cfg(feature = "ratatui")]
pub mod color;
#[cfg(feature = "ratatui")]
pub mod diff;
pub mod diffmodel;
pub mod diffstat;
pub mod doc;
pub mod glyph;
#[cfg(feature = "ratatui")]
pub mod link;
pub mod markdown;
#[cfg(feature = "ratatui")]
pub mod svg;
#[cfg(feature = "ratatui")]
pub mod theme;

#[cfg(feature = "ratatui")]
use glyph::Glyphs;
#[cfg(feature = "ratatui")]
use theme::Theme;

/// What rendering needs from the state besides the cell itself.
#[cfg(feature = "ratatui")]
#[derive(Debug, Clone, Copy)]
pub struct Look {
    pub width: u16,
    /// The syntect theme every highlighted span uses, already resolved from
    /// `tui.syntax_theme` and `tui.theme`.
    pub theme: &'static str,
    /// What the terminal can print; `glyph::resolve` decided it.
    pub glyphs: Glyphs,
    /// `Ctrl+T`: thinking expanded rather than a one-line count.
    pub show_thinking: bool,
    /// `Ctrl+O`: diffs in full rather than their `+n −m` header.
    pub show_diffs: bool,
    /// `tui.diff` (T24.5): whether a wide viewport splits a diff in two.
    pub diff: diff::Mode,
    /// Ticks (100 ms) since start; drives the spinner and elapsed time.
    pub tick: u64,
    /// `tui.motion = reduced` (T24.7): nothing on screen moves by itself.
    pub still: bool,
    /// Leave `text::sanitize` markers where something was removed.
    pub marks: bool,
    /// The semantic colour tokens (T24.1) every styled span picks from,
    /// resolved from `tui.theme`/`NO_COLOR`; never a bare colour literal.
    pub colors: Theme,
    /// `Ctrl+E` (T24.4): whether *this* tool cell is the one still in the
    /// viewport that the key can reach. `None` — not that cell, `Ctrl+E`
    /// cannot open it, the fold line points at `/expand <id>` instead.
    /// `Some(open)` — it is; the fold line reads `Ctrl+E` and folding is
    /// skipped once `open` is true. Per-cell, so it is not part of the one
    /// `Look` a whole render pass shares; the caller (`view.rs`) sets it for
    /// the single index it applies to.
    pub expand_last: Option<bool>,
}
