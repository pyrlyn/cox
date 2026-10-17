// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! What the supervisor does when a worker exits (`docs/design/serve.md` D3).
//! Pure so the policy is tested without a process or a clock: the caller
//! measures the uptime and reads the exit status.

use std::time::Duration;

/// First retry delay: long enough for the kernel to free the dead worker's
/// session lock (D3), short enough that one crash is not noticed.
const BASE_DELAY: Duration = Duration::from_millis(250);

/// Delay ceiling, so a flapping worker never waits longer than half a minute.
const MAX_DELAY: Duration = Duration::from_secs(30);

/// A worker that fails this many times in a row is not coming back by itself
/// (D3); only an explicit `open` from a client starts a fresh count.
const MAX_FAILURES: u32 = 5;

/// A worker that lived this long was healthy, so its next crash is a new
/// incident and not part of a crash loop (D3).
const HEALTHY_UPTIME: Duration = Duration::from_secs(30);

/// The supervisor's answer to one worker exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Reopen the session with `resume` after this delay.
    Restart(Duration),
    /// Too many consecutive failures: list the session as failed.
    GiveUp,
    /// A clean exit: leave the worker stopped.
    Done,
}

/// Restart state for one worker: its run of consecutive failures.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Policy {
    failures: u32,
}

impl Policy {
    /// A fresh policy with no failures counted.
    pub fn new() -> Self {
        Self::default()
    }

    /// Consecutive failures counted so far.
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// Decide what to do after a worker that ran for `uptime` exited; `clean`
    /// is an exit after `end`, a supervisor shutdown or status 0.
    pub fn on_exit(&mut self, uptime: Duration, clean: bool) -> Decision {
        if clean {
            self.failures = 0;
            return Decision::Done;
        }
        if uptime >= HEALTHY_UPTIME {
            self.failures = 0;
        }
        self.failures = self.failures.saturating_add(1);
        if self.failures >= MAX_FAILURES {
            Decision::GiveUp
        } else {
            Decision::Restart(delay(self.failures))
        }
    }
}

/// The wait before the retry that follows the `failures`-th consecutive
/// failure: 250 ms, doubling, capped at 30 s.
pub fn delay(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1);
    BASE_DELAY
        .checked_mul(2u32.checked_pow(doublings).unwrap_or(u32::MAX))
        .map_or(MAX_DELAY, |d| d.min(MAX_DELAY))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHORT: Duration = Duration::from_secs(1);

    #[test]
    fn delays_double_from_250ms_to_a_30s_cap() {
        let ms: Vec<u128> = (1..=4).map(|n| delay(n).as_millis()).collect();
        assert_eq!(ms, [250, 500, 1000, 2000]);
        assert_eq!(delay(5), Duration::from_secs(4));
        assert_eq!(delay(8), Duration::from_secs(30));
        assert_eq!(delay(u32::MAX), Duration::from_secs(30));

        let mut policy = Policy::new();
        assert_eq!(
            policy.on_exit(SHORT, false),
            Decision::Restart(Duration::from_millis(250))
        );
        assert_eq!(
            policy.on_exit(SHORT, false),
            Decision::Restart(Duration::from_millis(500))
        );
    }

    #[test]
    fn gives_up_after_five_consecutive_failures() {
        let mut policy = Policy::new();
        for _ in 0..4 {
            assert!(matches!(policy.on_exit(SHORT, false), Decision::Restart(_)));
        }
        assert_eq!(policy.on_exit(SHORT, false), Decision::GiveUp);
        assert_eq!(policy.failures(), 5);
    }

    #[test]
    fn thirty_seconds_of_life_resets_the_count() {
        let mut policy = Policy::new();
        for _ in 0..4 {
            policy.on_exit(SHORT, false);
        }
        assert_eq!(
            policy.on_exit(Duration::from_secs(30), false),
            Decision::Restart(Duration::from_millis(250))
        );
        assert_eq!(policy.failures(), 1);
        // One second short of healthy does not reset.
        assert_eq!(
            policy.on_exit(Duration::from_secs(29), false),
            Decision::Restart(Duration::from_millis(500))
        );
    }

    #[test]
    fn a_clean_exit_is_never_restarted() {
        let mut policy = Policy::new();
        policy.on_exit(SHORT, false);
        assert_eq!(policy.on_exit(SHORT, true), Decision::Done);
        assert_eq!(
            policy.on_exit(Duration::from_secs(3600), true),
            Decision::Done
        );
        assert_eq!(policy.failures(), 0);
    }
}
