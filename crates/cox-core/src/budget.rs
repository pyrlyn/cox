// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Session and monthly spend caps (plan.md D6h / T2.7). Pure so tests do
//! not need a provider; `Session` feeds it ledger totals.

/// What to do before the next provider call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Decision {
    /// Under the cap.
    Proceed,
    /// Crossed `warn_at`; emit `Notice(Budget)` once, then proceed.
    Warn,
    /// At or over the cap; emit `TurnDone{Budget}`.
    Stop,
}

/// `projected` is spend so far plus a pre-call estimate, both USD.
pub(crate) fn decide(projected: f64, cap: f64, warn_at: f64, already_warned: bool) -> Decision {
    if cap <= 0.0 || projected >= cap {
        return Decision::Stop;
    }
    if !already_warned && projected >= warn_at * cap {
        return Decision::Warn;
    }
    Decision::Proceed
}

/// Whether this usage row counts against the cap.
pub(crate) fn counts(tier: cox_protocol::types::Tier, cheap_counts: bool) -> bool {
    cheap_counts || tier != cox_protocol::types::Tier::Cheap
}

#[cfg(test)]
mod tests {
    use super::*;
    use cox_protocol::types::Tier;

    #[test]
    fn budget_stops_when_spent_at_cap() {
        assert_eq!(decide(5.0, 5.0, 0.8, false), Decision::Stop);
        assert_eq!(decide(0.1, 0.0, 0.8, false), Decision::Stop);
    }

    #[test]
    fn budget_warns_once_at_threshold() {
        assert_eq!(decide(4.0, 5.0, 0.8, false), Decision::Warn);
        assert_eq!(decide(4.0, 5.0, 0.8, true), Decision::Proceed);
        assert_eq!(decide(1.0, 5.0, 0.8, false), Decision::Proceed);
    }

    #[test]
    fn budget_cheap_excluded_when_configured() {
        assert!(!counts(Tier::Cheap, false));
        assert!(counts(Tier::Cheap, true));
        assert!(counts(Tier::Code, false));
    }
}
