// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! What the status line reads from `State`: `Status` (model, context, cost,
//! budget), `GitStatus` (the working tree) and `Notify` (`tui.notify`, when the
//! terminal rings). Plain data the runtime and `crate::status` read; apart
//! from `State` so the line's inputs sit in one place.

use cox_protocol::types::{ContextBreakdown, Effort, SandboxMode, Tier};

/// What the status line shows; filled from `TurnStarted`/`Usage`.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub model: String,
    pub tier: Option<Tier>,
    pub context_tokens: u32,
    /// What `ctx N%` is a share of: the model's window from the last
    /// `Event::ContextBreakdown` (A98), a 200k guess until one names it.
    pub context_window: u32,
    /// The last request's split (A98), which `/context` draws.
    pub context: Option<ContextBreakdown>,
    pub cost_usd: f64,
    pub sandbox: SandboxMode,
    pub busy: bool,
    /// Last call's cache share 0..=1 (T8.3), shown as `cache N%`.
    pub cache_ratio: f64,
    /// Session spend cap, in USD (T28.1); the binary sets it from
    /// `budget.session_usd`, so `$` names the spend over the cap.
    pub budget_cap_usd: f64,
    /// Fraction of the cap that warns (T28.1); the binary sets it from
    /// `budget.warn_at`, and the cost segment turns `theme.warn` past it.
    pub budget_warn_at: f64,
    /// `/effort` override for the session (T28.1); `SetEffort` keeps it here
    /// next to the mode the composer already shows, and the line badges it.
    pub effort: Option<Effort>,
}

impl Status {
    /// The context `ctx N%` and `/context` count (A98): the last call's
    /// reported context, else the core's estimate for the request about to
    /// go out — `ContextBreakdown::used`, the desktop meter's rule too.
    pub fn context_used(&self) -> u32 {
        self.context
            .map_or(self.context_tokens, |b| b.used(self.context_tokens))
    }
}

/// What the status line shows of the working tree; a mirror of
/// `cox_tools::git::Status` so this crate keeps no `cox-tools` dependency
/// (T15.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStatus {
    pub branch: String,
    pub added: usize,
    pub removed: usize,
}

/// `tui.notify` (T23.5): when a finished turn, an approval or a question
/// rings the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notify {
    /// Only while the terminal is unfocused.
    Auto,
    Always,
    Off,
}

impl Notify {
    /// An unknown value is `Auto`, like `diff::Mode::parse`.
    pub fn parse(s: &str) -> Notify {
        match s {
            "always" => Notify::Always,
            "off" => Notify::Off,
            _ => Notify::Auto,
        }
    }
}
