// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T67.11: `cox run -p --json-schema` against the real binary and the
//! scripted provider. The schema is a file the test writes; the model is
//! the scenario.

use std::path::Path;

use assert_cmd::Command;
use serde_json::Value;

const SCHEMA: &str = r#"{"type":"object","required":["ok"],"properties":{"ok":{"type":"boolean"}},"additionalProperties":false}"#;

fn cox(work: &Path, home: &Path, scenario: &Path, schema: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cox"));
    cmd.current_dir(work)
        .env("COX_HOME", home)
        .env("HOME", home)
        .env("COX_PROVIDER", "scripted")
        .env("COX_SCENARIO", scenario)
        .args([
            "--cwd",
            work.to_str().unwrap(),
            "run",
            "-p",
            "answer as JSON",
        ])
        .args(["--json-schema", schema.to_str().unwrap()])
        .args(["--output-format", "stream-json"]);
    cmd
}

fn write_scenario(home: &Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = home.join(name);
    std::fs::write(&path, body).unwrap();
    path
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

    fn turn_done(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| l["type"] == "turn_done")
            .count()
    }

    fn result(&self) -> &Value {
        self.lines
            .iter()
            .rev()
            .find(|l| l["type"] == "result")
            .unwrap()
    }
}

fn layout() -> (tempfile::TempDir, tempfile::TempDir, std::path::PathBuf) {
    let (work, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let schema = home.path().join("answer.schema.json");
    std::fs::write(&schema, SCHEMA).unwrap();
    (work, home, schema)
}

#[test]
fn json_schema_accepts_a_valid_answer_on_the_first_turn() {
    let (work, home, schema) = layout();
    let scenario = write_scenario(
        home.path(),
        "ok.toml",
        "[[turn]]\ntext = \"{\\\"ok\\\":true}\"\n",
    );
    let run = Run::of(&mut cox(work.path(), home.path(), &scenario, &schema));
    assert_eq!(run.turn_done(), 1, "{:?}", run.lines);
    assert_eq!(run.result()["result"], "{\"ok\":true}");
    assert_eq!(run.result()["is_error"], false);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(!run.stderr.contains("does not match"), "{}", run.stderr);
}

#[test]
fn json_schema_retries_once_and_accepts_the_second_answer() {
    let (work, home, schema) = layout();
    let scenario = write_scenario(
        home.path(),
        "retry.toml",
        "[[turn]]\ntext = \"not json\"\n\n[[turn]]\ntext = \"{\\\"ok\\\":true}\"\n",
    );
    let run = Run::of(&mut cox(work.path(), home.path(), &scenario, &schema));
    assert_eq!(run.turn_done(), 2, "{:?}", run.lines);
    assert_eq!(run.result()["result"], "{\"ok\":true}");
    assert_eq!(run.code, 0, "{}", run.stderr);
}

#[test]
fn json_schema_exits_nonzero_after_two_failures() {
    let (work, home, schema) = layout();
    let scenario = write_scenario(
        home.path(),
        "bad.toml",
        "[[turn]]\ntext = \"nope\"\n\n[[turn]]\ntext = \"{\\\"ok\\\":\\\"yes\\\"}\"\n",
    );
    let run = Run::of(&mut cox(work.path(), home.path(), &scenario, &schema));
    assert_eq!(run.turn_done(), 2, "{:?}", run.lines);
    assert_eq!(run.result()["result"], "{\"ok\":\"yes\"}");
    assert_eq!(run.result()["is_error"], true);
    assert_ne!(run.code, 0);
    assert!(
        run.stderr.contains("does not match --json-schema"),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("is not of type \"boolean\""),
        "{}",
        run.stderr
    );
}
