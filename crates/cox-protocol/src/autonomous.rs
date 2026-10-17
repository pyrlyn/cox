// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The contract of a gate-driven headless run (T66.9): the policy that
//! bounds it, the `GateRunner` seam that runs a gate command, a gate's
//! result and why the run stopped. Kept here because `cox-core` decides
//! (`cox_core::autonomous`) while `cox-tools` runs the shell (T66.11), and
//! `cox-core` may not depend on a crate that starts a process.

use std::time::Duration;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::StopReason;

/// Limits and gates of one autonomous run. Field names match the
/// `[autonomous]` config table T66.10 adds, so that table can be this type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct AutonomousPolicy {
    /// Shell commands run in order after each turn; the first failure wins.
    pub gates: Vec<String>,
    /// Turns the driver may start after the user's own.
    pub max_continuations: u32,
    /// Turns in the whole run, the user's included.
    pub max_turns: u32,
    /// Tokens the run's turns may use in total.
    pub max_tokens: u64,
    /// Wall-clock seconds the run may take.
    pub timeout_secs: u64,
    /// Failed checks of one gate the run may retry before it gives up.
    pub gate_retries: u32,
    /// Seconds one gate command may run before it counts as failed.
    pub gate_timeout_secs: u64,
}

impl Default for AutonomousPolicy {
    /// The defaults prime-agent ships (`pa-core/src/autonomous/mod.rs`),
    /// with no gates: a user must name at least one.
    fn default() -> Self {
        Self {
            gates: Vec::new(),
            max_continuations: 3,
            max_turns: 12,
            max_tokens: 80_000,
            timeout_secs: 30 * 60,
            gate_retries: 3,
            gate_timeout_secs: 5 * 60,
        }
    }
}

impl AutonomousPolicy {
    /// Without a gate nothing could ever fail, so every run would "pass"
    /// after one turn; that is a misconfiguration, not a result.
    pub fn validate(&self) -> Result<(), AutonomousError> {
        if self.gates.is_empty() {
            return Err(AutonomousError::NoGates);
        }
        // A blank command would run an empty shell, which always passes.
        match self.gates.iter().position(|g| g.trim().is_empty()) {
            Some(index) => Err(AutonomousError::BlankGate { index }),
            None => Ok(()),
        }
    }

    /// `gate_timeout_secs` as the runner takes it.
    pub fn gate_timeout(&self) -> Duration {
        Duration::from_secs(self.gate_timeout_secs)
    }
}

/// Why an autonomous policy cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AutonomousError {
    /// `gates` is empty.
    #[error("autonomous mode needs at least one gate command")]
    NoGates,
    /// The gate at `index` is blank.
    #[error("autonomous gate {index} is blank")]
    BlankGate {
        /// Zero-based position in `gates`.
        index: usize,
    },
}

/// One gate command's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GateResult {
    /// The command as configured.
    pub command: String,
    /// Exit status zero within the gate timeout.
    pub passed: bool,
    /// The capped end of its output; the runner archives the whole of it.
    pub tail: String,
}

/// Runs one gate command. Infallible on purpose: a spawn failure, a
/// sandbox denial or a timeout is a failed gate whose `tail` says why, so
/// the driver's retry and no-progress limits bound it like any failure.
#[async_trait]
pub trait GateRunner: Send + Sync {
    /// Run `command` and report whether it passed within `timeout`.
    async fn run(&self, command: &str, timeout: Duration) -> GateResult;
}

/// Why an autonomous run stopped. `GatesPassed` means only that every gate
/// command exited zero; it is not task success, so no surface reports it
/// as "done" or "success" (T66.9 step 4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum AutonomousStop {
    /// Every gate command exited zero.
    GatesPassed,
    /// `command` failed more than `gate_retries` times.
    GateRetriesExhausted {
        /// The gate that kept failing.
        command: String,
    },
    /// Continuations changed no file and repeated the same gate result.
    NoProgress,
    /// `max_continuations` was reached.
    MaxContinuations,
    /// `max_turns` was reached.
    MaxTurns,
    /// `max_tokens` was reached.
    MaxTokens,
    /// `timeout_secs` was reached.
    Timeout,
    /// The turn itself ended in a way that never continues: an error, an
    /// interrupt, a refusal or the USD cap.
    TurnEnded {
        /// The turn's own stop reason.
        stop: StopReason,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_has_prime_agent_limits_and_no_gates() {
        let p = AutonomousPolicy::default();
        assert_eq!(
            (
                p.max_continuations,
                p.max_turns,
                p.max_tokens,
                p.gate_retries
            ),
            (3, 12, 80_000, 3)
        );
        assert_eq!(p.timeout_secs, 1800);
        assert_eq!(p.gate_timeout(), Duration::from_secs(300));
        assert!(p.gates.is_empty());
    }

    #[test]
    fn policy_without_a_real_gate_is_a_config_error() {
        let mut p = AutonomousPolicy::default();
        assert_eq!(p.validate(), Err(AutonomousError::NoGates));
        p.gates = vec!["cargo test".into(), "  ".into()];
        assert_eq!(p.validate(), Err(AutonomousError::BlankGate { index: 1 }));
        p.gates = vec!["cargo test".into()];
        assert_eq!(p.validate(), Ok(()));
    }

    #[test]
    fn stop_reason_serializes_with_a_snake_case_tag() {
        let json = serde_json::to_value(AutonomousStop::GateRetriesExhausted {
            command: "make test".into(),
        })
        .expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({"reason": "gate_retries_exhausted", "command": "make test"})
        );
    }
}
