// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T66.9's Done-when claims for the autonomous driver, run against a
//! scripted `GateRunner` so no shell, clock or provider is involved.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use cox_core::autonomous::{Driver, Next, State, TurnReport};
use cox_protocol::autonomous::{
    AutonomousError, AutonomousPolicy, AutonomousStop, GateResult, GateRunner,
};
use cox_protocol::types::StopReason;
use rstest::rstest;

/// Answers each `run` with the next scripted pass/fail and records the
/// commands it was asked to run.
struct ScriptedGates {
    script: Mutex<VecDeque<bool>>,
    ran: Mutex<Vec<String>>,
}

impl ScriptedGates {
    fn new(script: &[bool]) -> Self {
        Self {
            script: Mutex::new(script.iter().copied().collect()),
            ran: Mutex::new(Vec::new()),
        }
    }

    fn ran(&self) -> Vec<String> {
        self.ran.lock().expect("lock").clone()
    }
}

#[async_trait]
impl GateRunner for ScriptedGates {
    async fn run(&self, command: &str, _timeout: Duration) -> GateResult {
        self.ran.lock().expect("lock").push(command.to_owned());
        let passed = self
            .script
            .lock()
            .expect("lock")
            .pop_front()
            .expect("a scripted gate result is left");
        GateResult {
            command: command.to_owned(),
            passed,
            tail: if passed {
                String::new()
            } else {
                "1 test failed".into()
            },
        }
    }
}

fn driver(gates: &[&str], tweak: impl FnOnce(&mut AutonomousPolicy)) -> Driver {
    let mut policy = AutonomousPolicy {
        gates: gates.iter().map(|g| (*g).to_owned()).collect(),
        ..AutonomousPolicy::default()
    };
    tweak(&mut policy);
    Driver::new(policy).expect("valid policy")
}

fn turn(stop: StopReason, tokens: u64, files_changed: bool) -> TurnReport {
    TurnReport {
        stop,
        tokens,
        files_changed,
        elapsed: Duration::from_secs(1),
    }
}

fn edited() -> TurnReport {
    turn(StopReason::EndTurn, 100, true)
}

#[tokio::test]
async fn a_passing_gate_stops_the_run_with_gates_passed() {
    let d = driver(&["cargo test"], |_| {});
    let gates = ScriptedGates::new(&[true]);
    let mut state = State::default();

    let next = d.decide(&mut state, &edited(), &gates).await;

    assert_eq!(next, Next::Stop(AutonomousStop::GatesPassed));
    assert_eq!(state.continuations, 0);
}

#[tokio::test]
async fn a_fail_then_a_pass_stops_on_the_second_try() {
    let d = driver(&["cargo test"], |_| {});
    let gates = ScriptedGates::new(&[false, true]);
    let mut state = State::default();

    let Next::Continue(text) = d.decide(&mut state, &edited(), &gates).await else {
        panic!("a failing gate with retries left continues");
    };
    assert!(text.contains("`cargo test`"), "{text}");
    assert!(text.contains("1 test failed"), "{text}");

    let next = d.decide(&mut state, &edited(), &gates).await;
    assert_eq!(next, Next::Stop(AutonomousStop::GatesPassed));
    assert_eq!((state.turns, state.continuations), (2, 1));
}

#[tokio::test]
async fn the_token_limit_stops_the_run_while_a_gate_still_fails() {
    let d = driver(&["cargo test"], |p| p.max_tokens = 1_000);
    let gates = ScriptedGates::new(&[false]);
    let mut state = State::default();

    let next = d
        .decide(&mut state, &turn(StopReason::EndTurn, 1_200, true), &gates)
        .await;

    assert_eq!(next, Next::Stop(AutonomousStop::MaxTokens));
}

#[rstest]
#[case::an_error(StopReason::Error)]
#[case::an_interrupt(StopReason::Interrupted)]
#[case::a_refusal(StopReason::Refusal { detail: "no".into() })]
#[case::the_usd_cap(StopReason::Budget)]
#[tokio::test]
async fn a_turn_that_ended_this_way_does_not_continue_or_run_a_gate(#[case] stop: StopReason) {
    let d = driver(&["cargo test"], |_| {});
    let gates = ScriptedGates::new(&[]);
    let mut state = State::default();

    let next = d
        .decide(&mut state, &turn(stop.clone(), 10, true), &gates)
        .await;

    assert_eq!(next, Next::Stop(AutonomousStop::TurnEnded { stop }));
    assert!(gates.ran().is_empty());
}

#[tokio::test]
async fn two_no_progress_continuations_stop_the_run() {
    let d = driver(&["cargo test"], |_| {});
    let gates = ScriptedGates::new(&[false, false, false]);
    let mut state = State::default();
    let idle = turn(StopReason::EndTurn, 100, false);

    assert!(matches!(
        d.decide(&mut state, &idle, &gates).await,
        Next::Continue(_)
    ));
    assert!(matches!(
        d.decide(&mut state, &idle, &gates).await,
        Next::Continue(_)
    ));
    assert_eq!(state.no_progress_streak, 1);
    let next = d.decide(&mut state, &idle, &gates).await;

    assert_eq!(next, Next::Stop(AutonomousStop::NoProgress));
    assert_eq!(state.no_progress_streak, 2);
}

#[tokio::test]
async fn a_changed_file_resets_the_no_progress_streak() {
    let d = driver(&["cargo test"], |p| {
        p.max_continuations = 10;
        p.gate_retries = 10;
    });
    let gates = ScriptedGates::new(&[false, false, false, false]);
    let mut state = State::default();
    let idle = turn(StopReason::EndTurn, 100, false);

    d.decide(&mut state, &idle, &gates).await;
    d.decide(&mut state, &idle, &gates).await;
    d.decide(&mut state, &edited(), &gates).await;
    assert_eq!(state.no_progress_streak, 0);
    assert!(matches!(
        d.decide(&mut state, &idle, &gates).await,
        Next::Continue(_)
    ));
}

#[tokio::test]
async fn the_first_failing_gate_wins_and_later_gates_do_not_run() {
    let d = driver(&["fmt", "test", "lint"], |_| {});
    let gates = ScriptedGates::new(&[true, false]);
    let mut state = State::default();

    let Next::Continue(text) = d.decide(&mut state, &edited(), &gates).await else {
        panic!("a failing gate continues");
    };

    assert!(text.contains("`test`"), "{text}");
    assert_eq!(gates.ran(), ["fmt", "test"]);
}

#[tokio::test]
async fn exhausted_retries_stop_the_run_naming_the_gate() {
    let d = driver(&["cargo test"], |p| {
        p.gate_retries = 1;
        p.max_continuations = 10;
    });
    let gates = ScriptedGates::new(&[false, false]);
    let mut state = State::default();

    assert!(matches!(
        d.decide(&mut state, &edited(), &gates).await,
        Next::Continue(_)
    ));
    let next = d.decide(&mut state, &edited(), &gates).await;

    assert_eq!(
        next,
        Next::Stop(AutonomousStop::GateRetriesExhausted {
            command: "cargo test".into()
        })
    );
}

#[tokio::test]
async fn the_time_limit_stops_the_run() {
    let d = driver(&["cargo test"], |p| p.timeout_secs = 60);
    let gates = ScriptedGates::new(&[false]);
    let mut state = State::default();
    let late = TurnReport {
        elapsed: Duration::from_secs(61),
        ..edited()
    };

    let next = d.decide(&mut state, &late, &gates).await;

    assert_eq!(next, Next::Stop(AutonomousStop::Timeout));
}

#[tokio::test]
async fn a_pass_at_a_limit_still_reports_gates_passed() {
    let d = driver(&["cargo test"], |p| p.max_turns = 1);
    let gates = ScriptedGates::new(&[true]);
    let mut state = State::default();

    let next = d.decide(&mut state, &edited(), &gates).await;

    assert_eq!(next, Next::Stop(AutonomousStop::GatesPassed));
}

#[test]
fn the_driver_refuses_a_policy_without_gates() {
    let err = Driver::new(AutonomousPolicy::default()).expect_err("no gates");
    assert_eq!(err, AutonomousError::NoGates);
}
