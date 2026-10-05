// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The ratatui app in TEA form (`State`, `update`, `view`); all terminal
//! output goes through here. Separate from `cox-core` so the agent loop has
//! no notion of a terminal and the TUI can be tested by feeding it `Event`s.

// why: this crate was not under `missing_docs` before the workspace lints;
// drop this allow once its public items are documented.
#![allow(missing_docs)]

pub mod app;
pub mod banner;
pub mod cells;
pub mod commands;
pub mod composer;
pub mod item_render;
pub mod keymap;
pub mod modal;
pub mod picker;
pub mod plugin_ui;
pub mod state;
pub mod status;
pub mod tasks;
pub mod term;
pub mod theme_editor;
pub mod view;
pub mod vim;
pub mod voice;

/// T32.2: the renderers moved to `cox-render` (dependency rule (a) in
/// `docs/design/crates.md`: syntect, two-face, pulldown-cmark and
/// terminal-colorsaurus); re-exported at their old paths so `crate::theme`,
/// `cox_tui::diff` and the rest keep working.
pub use cox_render::{color, diff, glyph, link, markdown, svg, theme};

/// T32.1: `text::sanitize` moved to its own crate (guard (b), reuse (d) —
/// `docs/design/crates.md`); re-exported here at the old path so
/// `cox_tui::text::sanitize` keeps working for existing callers.
pub use cox_sanitize as text;
