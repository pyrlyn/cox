// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T66.11: `cox run -p --autonomous` against the real binary, the scripted
//! provider and a scratch `COX_HOME`. The gates are real shell commands run
//! through `bash`'s sandboxed runner; only the model is scripted.

use std::path::Path;

use assert_cmd::Command;
use serde_json::Value;

const FIX_THEN_DONE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/scenarios/autonomous_fix_then_done.toml"
);
const TALK_ONLY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/scenarios/autonomous_talk_only.toml"
);

fn cox(work: &Path, home: &Path, scenario: &str) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cox"));
    cmd.current_dir(work)
        .env("COX_HOME", home)
        .env("HOME", home)
        .env("COX_PROVIDER", "scripted")
        .env("COX_SCENARIO", scenario)
        .args(["--cwd", work.to_str().unwrap(), "run", "-p", "fix it"])
        .args(["--autonomous", "--output-format", "stream-json"]);
    cmd
}

/// The user's own config, the only place `[autonomous]` is read from.
fn config(home: &Path, toml: &str) {
    std::fs::write(home.join("config.toml"), toml).unwrap();
}

struct Run {
    code: i32,
    lines: Vec<Value>,
    stderr: String,
}

impl Run {
    fn of(cmd: &mut Command) -> Self {
        let out = cmd.output().unwrap();
        let stdout = String::from_utf8(out.stdout).unwrap();
        Self {
            code: out.status.code().unwrap(),
            lines: stdout
                .lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect(),
            stderr: String::from_utf8(out.stderr).unwrap(),
        }
    }

    fn count(&self, kind: &str) -> usize {
        self.lines.iter().filter(|l| l["type"] == kind).count()
    }

    fn decisions(&self) -> Vec<&Value> {
        self.lines
            .iter()
            .filter(|l| l["type"] == "autonomous")
            .collect()
    }
}

#[test]
fn autonomous_a_failing_gate_continues_the_run_and_a_passing_gate_stops_it() {
    let (work, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    config(
        home.path(),
        "[autonomous]\ngates = [\"test -f marker.txt\"]\n",
    );
    let run =
        Run::of(cox(work.path(), home.path(), FIX_THEN_DONE).args(["--permission-mode", "auto"]));
    // Two user turns: the prompt, then the driver's continuation.
    assert_eq!(run.count("turn_done"), 2, "{:?}", run.lines);
    let decisions = run.decisions();
    let seen: Vec<_> = decisions
        .iter()
        .map(|d| d["decision"].as_str().unwrap())
        .collect();
    assert_eq!(seen, ["continue", "stop"]);
    assert_eq!(decisions[1]["stop"]["reason"], "gates_passed");
    let result = run.lines.last().unwrap();
    assert_eq!(result["type"], "result");
    assert_eq!(result["autonomous"]["reason"], "gates_passed");
    assert!(work.path().join("marker.txt").exists());
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(
        run.stderr.contains("autonomous run stopped"),
        "{}",
        run.stderr
    );
}

#[test]
fn autonomous_a_gate_that_already_passes_ends_the_run_after_one_turn() {
    let (work, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    config(home.path(), "[autonomous]\ngates = [\"true\"]\n");
    let run = Run::of(&mut cox(work.path(), home.path(), TALK_ONLY));
    assert_eq!(run.count("turn_done"), 1);
    assert_eq!(run.decisions().len(), 1);
    assert_eq!(run.decisions()[0]["stop"]["reason"], "gates_passed");
    assert_eq!(run.code, 0, "{}", run.stderr);
}

#[test]
fn autonomous_a_gate_that_keeps_failing_stops_the_run_and_exits_nonzero() {
    let (work, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    config(
        home.path(),
        "[autonomous]\ngates = [\"false\"]\ngate_retries = 1\n",
    );
    let run = Run::of(&mut cox(work.path(), home.path(), TALK_ONLY));
    assert_eq!(run.count("turn_done"), 2);
    let stop = run.decisions().last().unwrap()["stop"].clone();
    assert_eq!(stop["reason"], "gate_retries_exhausted");
    assert_eq!(stop["command"], "false");
    assert_eq!(run.code, 1, "a run that gave up is not a success");
}

#[test]
fn autonomous_without_a_gate_is_refused_before_any_turn() {
    let (work, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let run = Run::of(&mut cox(work.path(), home.path(), TALK_ONLY));
    assert_ne!(run.code, 0);
    assert!(run.lines.is_empty(), "{:?}", run.lines);
    assert!(run.stderr.contains("at least one gate"), "{}", run.stderr);
}

#[test]
fn autonomous_gates_set_by_a_project_config_are_not_run() {
    let (work, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    config(home.path(), "[autonomous]\ngates = [\"true\"]\n");
    // The project layer is read from the git root's `.cox/`.
    std::fs::create_dir_all(work.path().join(".git")).unwrap();
    std::fs::create_dir(work.path().join(".cox")).unwrap();
    std::fs::write(
        work.path().join(".cox/config.toml"),
        "[autonomous]\ngates = [\"touch pwned\"]\n",
    )
    .unwrap();
    let run = Run::of(&mut cox(work.path(), home.path(), TALK_ONLY));
    assert_eq!(run.decisions()[0]["stop"]["reason"], "gates_passed");
    assert!(!work.path().join("pwned").exists());
}
