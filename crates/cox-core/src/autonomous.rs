// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The autonomous driver (T66.9): after each headless turn it decides
//! whether the run starts another one. It continues only toward a failing
//! gate and only within its own turn, token, time, retry and no-progress
//! limits. Separate from `budget`, which stays the USD cap and reaches this
//! driver only as a turn that ended with `StopReason::Budget`. Deterministic:
//! the gate runner is a trait object and the clock is a value the caller
//! passes, so tests drive it with a script instead of a shell.

use std::collections::BTreeMap;
use std::time::Duration;

use cox_protocol::autonomous::{
    AutonomousError, AutonomousPolicy, AutonomousStop, GateResult, GateRunner,
};
use cox_protocol::types::StopReason;

/// What one finished turn tells the driver.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnReport {
    /// How the turn ended.
    pub stop: StopReason,
    /// Tokens the turn used (input plus output, as the ledger counts them).
    pub tokens: u64,
    /// Whether the turn changed any file in the workspace.
    pub files_changed: bool,
    /// Wall-clock time since the run started, read by the caller.
    pub elapsed: Duration,
}

/// The run's counters. One per run; the caller keeps it between turns.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    /// Turns finished, the user's own included.
    pub turns: u32,
    /// Turns the driver started.
    pub continuations: u32,
    /// Tokens used by every finished turn.
    pub tokens: u64,
    /// Consecutive continuations that changed no file and repeated the
    /// previous gate result.
    pub no_progress_streak: u32,
    /// Failed checks per gate command; a map so the count is per gate and
    /// iteration order stays deterministic.
    pub failures: BTreeMap<String, u32>,
    /// The last failing gate result, to tell a repeat from progress.
    pub last_gate: Option<GateResult>,
}

/// The driver's verdict after a turn.
#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    /// Start another turn with this user text.
    Continue(String),
    /// End the run.
    Stop(AutonomousStop),
}

/// Two continuations with nothing changed are enough to call a loop stuck:
/// one repeat can be the model reading before it edits, two cannot.
const NO_PROGRESS_LIMIT: u32 = 2;

/// A validated policy. Constructing one is the only way to call `decide`,
/// so a gateless policy cannot reach the loop.
#[derive(Debug, Clone)]
pub struct Driver {
    policy: AutonomousPolicy,
}

impl Driver {
    /// Reject a policy without a usable gate (T66.9 step 5).
    pub fn new(policy: AutonomousPolicy) -> Result<Self, AutonomousError> {
        policy.validate()?;
        Ok(Self { policy })
    }

    /// The policy this driver enforces.
    pub fn policy(&self) -> &AutonomousPolicy {
        &self.policy
    }

    /// Fold `turn` into `state` and decide what follows it.
    ///
    /// Order matters. A turn that must not continue stops before any gate
    /// runs, so a cancelled run starts no process. Gates come next because
    /// a pass ends the run even at a limit. Then the run's own limits, then
    /// the failing gate's retries, then the no-progress streak.
    pub async fn decide<R>(&self, state: &mut State, turn: &TurnReport, gates: &R) -> Next
    where
        R: GateRunner + ?Sized,
    {
        state.turns = state.turns.saturating_add(1);
        state.tokens = state.tokens.saturating_add(turn.tokens);

        if never_continues(&turn.stop) {
            return Next::Stop(AutonomousStop::TurnEnded {
                stop: turn.stop.clone(),
            });
        }

        let Some(failed) = self.first_failure(gates).await else {
            return Next::Stop(AutonomousStop::GatesPassed);
        };

        let attempts = {
            let n = state.failures.entry(failed.command.clone()).or_insert(0);
            *n = n.saturating_add(1);
            *n
        };
        let repeated = !turn.files_changed && state.last_gate.as_ref() == Some(&failed);
        state.no_progress_streak = if repeated {
            state.no_progress_streak.saturating_add(1)
        } else {
            0
        };
        state.last_gate = Some(failed.clone());

        if let Some(stop) = self.limit_reached(state, turn.elapsed) {
            return Next::Stop(stop);
        }
        if attempts > self.policy.gate_retries {
            return Next::Stop(AutonomousStop::GateRetriesExhausted {
                command: failed.command,
            });
        }
        if state.no_progress_streak >= NO_PROGRESS_LIMIT {
            return Next::Stop(AutonomousStop::NoProgress);
        }

        state.continuations = state.continuations.saturating_add(1);
        Next::Continue(continuation_text(
            &failed,
            attempts,
            self.policy.gate_retries.saturating_add(1),
        ))
    }

    /// Gates run in order and the first failure wins; later gates would
    /// only report on code the model is about to change anyway.
    async fn first_failure<R>(&self, gates: &R) -> Option<GateResult>
    where
        R: GateRunner + ?Sized,
    {
        let timeout = self.policy.gate_timeout();
        for command in &self.policy.gates {
            let result = gates.run(command, timeout).await;
            if !result.passed {
                return Some(result);
            }
        }
        None
    }

    fn limit_reached(&self, state: &State, elapsed: Duration) -> Option<AutonomousStop> {
        let p = &self.policy;
        if state.turns >= p.max_turns {
            Some(AutonomousStop::MaxTurns)
        } else if state.tokens >= p.max_tokens {
            Some(AutonomousStop::MaxTokens)
        } else if elapsed >= Duration::from_secs(p.timeout_secs) {
            Some(AutonomousStop::Timeout)
        } else if state.continuations >= p.max_continuations {
            Some(AutonomousStop::MaxContinuations)
        } else {
            None
        }
    }
}

/// An error, an interrupt, a refusal or the USD cap is the user's or the
/// provider's decision to stop; a gate failure cannot override it (idea:
/// prime-agent `should_autonomously_continue`).
fn never_continues(stop: &StopReason) -> bool {
    matches!(
        stop,
        StopReason::Error
            | StopReason::Interrupted
            | StopReason::Refusal { .. }
            | StopReason::Budget
    )
}

/// The next user turn: the failing command and the tail of its output. The
/// runner already capped and archived that tail (T66.11), so it is quoted
/// as given.
fn continuation_text(failed: &GateResult, attempt: u32, of: u32) -> String {
    format!(
        "The gate `{}` failed (check {attempt} of {of}). The end of its output:\n\n{}\n\n\
         Fix the cause. The gates run again when this turn ends.",
        failed.command, failed.tail
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn driver(p: impl FnOnce(&mut AutonomousPolicy)) -> Driver {
        let mut policy = AutonomousPolicy {
            gates: vec!["t".into()],
            ..AutonomousPolicy::default()
        };
        p(&mut policy);
        Driver::new(policy).expect("valid policy")
    }

    fn state(turns: u32, continuations: u32, tokens: u64) -> State {
        State {
            turns,
            continuations,
            tokens,
            ..State::default()
        }
    }

    #[test]
    fn max_turns_is_checked_before_the_other_limits() {
        let d = driver(|p| {
            p.max_turns = 2;
            p.max_tokens = 10;
        });
        assert_eq!(
            d.limit_reached(&state(2, 0, 10), Duration::ZERO),
            Some(AutonomousStop::MaxTurns)
        );
    }

    #[test]
    fn each_limit_stops_under_its_own_reason() {
        let d = driver(|p| {
            p.max_turns = 5;
            p.max_tokens = 100;
            p.timeout_secs = 60;
            p.max_continuations = 2;
        });
        let quiet = Duration::from_secs(1);
        assert_eq!(d.limit_reached(&state(1, 0, 1), quiet), None);
        assert_eq!(
            d.limit_reached(&state(1, 0, 100), quiet),
            Some(AutonomousStop::MaxTokens)
        );
        assert_eq!(
            d.limit_reached(&state(1, 0, 1), Duration::from_secs(60)),
            Some(AutonomousStop::Timeout)
        );
        assert_eq!(
            d.limit_reached(&state(1, 2, 1), quiet),
            Some(AutonomousStop::MaxContinuations)
        );
    }

    #[test]
    fn max_turns_and_end_turn_may_continue_the_rest_never_do() {
        assert!(!never_continues(&StopReason::EndTurn));
        assert!(!never_continues(&StopReason::MaxTurns));
        assert!(never_continues(&StopReason::Budget));
        assert!(never_continues(&StopReason::Refusal {
            detail: String::new()
        }));
    }
}
