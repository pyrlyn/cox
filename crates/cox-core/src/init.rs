// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `/init` (T25.6): an `AGENTS.md` skeleton for the repo.
//!
//! The manifest table and the template are pure (`stacks_for`, `render`) so
//! the interactive flow and `cox init` share them: the core gathers the file
//! list through the `bash`/`read` tools (the `turn::run_tools` path, never
//! the disk) and summarises the README on the cheap tier, while the `cox`
//! crate lists the directory itself. One template, two gatherers.

use cox_protocol::errors::CoreError;
use cox_protocol::ids::{CallId, TurnId};
use cox_protocol::types::{Event, Job, Level};
use serde_json::json;

use crate::session::Session;
use crate::side::Side;
use crate::turn::run_tools;

/// The file `/init` writes, at the session workspace root.
pub const FILENAME: &str = "AGENTS.md";

/// Fallback conventions when the repo has no README.
pub const NO_README: &str =
    "No README found. Add the repo's conventions here (build, style, review rules).";

/// The cheap-tier prompt that turns a README into convention bullets.
const SUMMARY_PROMPT: &str = "You are writing the Conventions section of a repo's AGENTS.md. \
    Summarise the README below into 3-6 terse bullets: build/test/lint commands, code style, \
    review rules. Skip marketing prose. Plain bullets only, no heading.";

/// The summary is short by construction; longer answers are cut, not billed on.
const MAX_SUMMARY_CHARS: usize = 1500;

/// One detected stack: which manifest proved it and the commands it implies.
#[derive(Debug, Clone, PartialEq)]
pub struct Stack {
    /// Human name for the commands table (`Rust`, `Node`, ...).
    pub stack: &'static str,
    /// Build command.
    pub build: &'static str,
    /// Test command.
    pub test: &'static str,
    /// Lint command.
    pub lint: &'static str,
}

/// `(manifest file, stack, build, test, lint)`; order is the commands-table order.
const KNOWN_MANIFESTS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "Cargo.toml",
        "Rust",
        "cargo build",
        "cargo test",
        "cargo clippy -- -D warnings",
    ),
    (
        "package.json",
        "Node",
        "npm run build",
        "npm test",
        "npm run lint",
    ),
    (
        "pyproject.toml",
        "Python",
        "python -m build",
        "pytest",
        "ruff check",
    ),
    (
        "go.mod",
        "Go",
        "go build ./...",
        "go test ./...",
        "go vet ./...",
    ),
    (
        "mise.toml",
        "mise",
        "mise run build",
        "mise run test",
        "mise run lint",
    ),
    ("justfile", "just", "just build", "just test", "just lint"),
];

/// The stacks whose manifest is in `files` (matched case-insensitively, so
/// `Justfile` counts), in commands-table order.
pub fn stacks_for(files: &[String]) -> Vec<Stack> {
    KNOWN_MANIFESTS
        .iter()
        .filter(|(manifest, ..)| files.iter().any(|f| f.eq_ignore_ascii_case(manifest)))
        .map(|(_, stack, build, test, lint)| Stack {
            stack,
            build,
            test,
            lint,
        })
        .collect()
}

/// Splits one `ls -1p` listing into `(name, is_dir)` pairs: a trailing `/`
/// marks a directory, the `[exit …]` trailer and blanks are skipped, and the
/// result is sorted so the template is stable. Entries may share a line —
/// the tool runs under a PTY, where `ls` columns with tabs — so fields
/// split on tab and run-together whitespace too.
pub fn parse_ls(output: &str) -> Vec<(String, bool)> {
    let mut entries = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('[') {
            continue;
        }
        for field in line.split(['\t', ' ']).filter(|f| !f.is_empty()) {
            if let Some(dir) = field.strip_suffix('/') {
                if !dir.is_empty() {
                    entries.push((dir.to_string(), true));
                }
            } else {
                entries.push((field.to_string(), false));
            }
        }
    }
    entries.sort();
    entries.dedup();
    entries
}

/// What the template needs: the repo name, top-level files and dirs, and
/// the README text when one was found.
#[derive(Debug, Clone, PartialEq)]
pub struct Scan {
    /// Directory name of the workspace root.
    pub name: String,
    /// Top-level file names, sorted.
    pub files: Vec<String>,
    /// Top-level directory names, sorted.
    pub dirs: Vec<String>,
    /// README text, when one was found.
    pub readme: Option<String>,
}

/// Builds a [`Scan`] from a directory listing (`(name, is_dir)` pairs).
pub fn scan_from_names(name: &str, entries: &[(String, bool)], readme: Option<String>) -> Scan {
    let mut files: Vec<String> = entries
        .iter()
        .filter(|(_, dir)| !dir)
        .map(|(name, _)| name.clone())
        .collect();
    let mut dirs: Vec<String> = entries
        .iter()
        .filter(|(_, dir)| *dir)
        .map(|(name, _)| name.clone())
        .collect();
    files.sort();
    dirs.sort();
    Scan {
        name: name.to_string(),
        files,
        dirs,
        readme,
    }
}

/// Best-effort gloss for a top-level directory; unknown names stay blank
/// rather than guessed at.
pub fn describe_dir(dir: &str) -> &'static str {
    match dir {
        "src" | "lib" | "app" => "sources",
        "crates" | "packages" | "libs" => "workspace crates / packages",
        "docs" => "documentation",
        "tests" | "test" | "e2e" => "tests",
        "fixtures" => "test fixtures",
        "scripts" => "scripts",
        "website" | "site" | "www" => "website",
        "config" => "config",
        "examples" => "examples",
        "benches" | "benchmarks" => "benchmarks",
        "evals" => "evals",
        ".cox" | ".claude" => "agent config",
        "assets" | "static" => "static assets",
        _ => "",
    }
}

/// Deterministic conventions fallback: the first twelve non-empty README
/// lines, or [`NO_README`] when nothing survives.
pub fn excerpt(readme: &str) -> String {
    let short: Vec<&str> = readme
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(12)
        .collect();
    if short.is_empty() {
        NO_README.to_string()
    } else {
        short.join("\n")
    }
}

/// Renders the skeleton: `# <name>`, the layout table, the commands table,
/// and the conventions text the caller (cheap summary or [`excerpt`]) chose.
pub fn render(scan: &Scan, conventions: &str) -> String {
    let mut out = format!(
        "# {}\n\n> Generated by `cox init`. Keep it short: this file is read into every session's context.\n\n## Layout\n\n| Path | What |\n| --- | --- |\n",
        scan.name
    );
    if scan.dirs.is_empty() {
        out.push_str("| — | no top-level directories |\n");
    } else {
        for dir in &scan.dirs {
            out.push_str(&format!("| {dir}/ | {} |\n", describe_dir(dir)));
        }
    }
    out.push_str("\n## Commands\n\n");
    let stacks = stacks_for(&scan.files);
    if stacks.is_empty() {
        out.push_str(
            "No manifests detected (`Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`, `mise.toml`, `justfile`).\n",
        );
    } else {
        out.push_str("| Task | Command |\n| --- | --- |\n");
        let multi = stacks.len() > 1;
        for stack in &stacks {
            let task = |kind: &str| {
                if multi {
                    format!("{} ({})", kind, stack.stack.to_lowercase())
                } else {
                    kind.to_string()
                }
            };
            out.push_str(&format!("| {} | `{}` |\n", task("build"), stack.build));
            out.push_str(&format!("| {} | `{}` |\n", task("test"), stack.test));
            out.push_str(&format!("| {} | `{}` |\n", task("lint"), stack.lint));
        }
    }
    out.push_str(&format!("\n## Conventions\n\n{conventions}\n"));
    out
}

impl Session {
    /// `/init [--force]` (T25.6): skeleton `AGENTS.md`, approved before it
    /// lands. Read-only discovery (`read` the target, `ls -p` the root,
    /// `read` the README) runs without asking; the README summary is one
    /// `summarize`-job call on the cheap tier; the `write` goes through the
    /// permission engine, so the approval modal shows it. An existing file
    /// without `force` is a warning, never a write. History is untouched.
    pub async fn run_init(&self, force: bool) -> Result<(), CoreError> {
        let turn = TurnId::new();
        let probe = run_tools(
            self,
            turn,
            vec![
                (CallId::new(), "read".into(), json!({"path": FILENAME})),
                // `ls -1p`: one entry per line even under the `bash` tool's
                // PTY, where a bare `ls -p` columns with tabs.
                (CallId::new(), "bash".into(), json!({"command": "ls -1p"})),
            ],
        )
        .await?;
        if !force && probe.first().is_some_and(|(_, result)| result.ok) {
            return self
                .emit(Event::Notice {
                    level: Level::Warn,
                    text: format!("{FILENAME} already exists; rerun with --force to overwrite"),
                })
                .await;
        }
        let listing = probe
            .get(1)
            .map(|(_, result)| result.visible.clone())
            .unwrap_or_default();
        let entries = parse_ls(&listing);
        let files: Vec<String> = entries
            .iter()
            .filter(|(_, dir)| !dir)
            .map(|(name, _)| name.clone())
            .collect();
        let readme_name = ["README.md", "readme.md", "README"]
            .iter()
            .find(|candidate| files.iter().any(|f| f == **candidate));
        let readme = match readme_name {
            Some(name) => {
                let found = run_tools(
                    self,
                    turn,
                    vec![(CallId::new(), "read".into(), json!({"path": name}))],
                )
                .await?;
                found
                    .into_iter()
                    .next()
                    .filter(|(_, result)| result.ok)
                    .map(|(_, result)| result.visible)
            }
            None => None,
        };
        let name = self
            .cwd
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repo".to_string());
        let scan = scan_from_names(&name, &entries, readme.clone());
        let conventions = match readme.as_deref() {
            Some(text) => self
                .cheap_summary(text)
                .await
                .unwrap_or_else(|| excerpt(text)),
            None => NO_README.to_string(),
        };
        let content = render(&scan, &conventions);
        let bytes = content.len();
        self.emit(Event::Notice {
            level: Level::Info,
            text: format!("proposed {FILENAME}:\n{content}"),
        })
        .await?;
        let done = run_tools(
            self,
            turn,
            vec![(
                CallId::new(),
                "write".into(),
                json!({"path": FILENAME, "content": content}),
            )],
        )
        .await?;
        let (level, text) = match done.into_iter().next() {
            Some((_, result)) if result.ok => {
                (Level::Info, format!("wrote {FILENAME} ({bytes} bytes)"))
            }
            Some((_, result)) => (
                Level::Warn,
                format!("{FILENAME} not written: {}", result.visible),
            ),
            None => (
                Level::Warn,
                format!("{FILENAME} not written: the write call never ran"),
            ),
        };
        self.emit(Event::Notice { level, text }).await
    }

    /// One `summarize`-job call over the README, in the ledger like any
    /// other cheap call; `None` on any failure, and the caller falls back
    /// to [`excerpt`] so a missing key never fails `/init`.
    async fn cheap_summary(&self, readme: &str) -> Option<String> {
        self.side_call(Side {
            job: Job::Summarize,
            turn: 0,
            system: SUMMARY_PROMPT,
            text: readme,
            max_tokens: 1024,
            max_chars: MAX_SUMMARY_CHARS,
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex as StdMutex};

    use cox_protocol::errors::ToolError;
    use cox_protocol::traits::{Tool, ToolCx};
    use cox_protocol::types::{
        Concurrency, Decision, Risk, Submission, Tier, ToolOutput, ToolSpec,
    };
    use cox_provider::scripted::Scripted;
    use serde_json::Value;
    use tokio::sync::mpsc;

    use crate::session::MemoryStore;

    /// The fake workspace one test drives: a fixed `ls -p` listing, file
    /// bytes by name, and every `write` recorded.
    #[derive(Default)]
    struct Fs {
        listing: String,
        files: HashMap<String, String>,
        writes: Vec<(String, String)>,
    }

    type Shared = Arc<StdMutex<Fs>>;

    fn spec(name: &str, risk: Risk) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: "init stub".into(),
            input_schema: serde_json::json!({"type": "object"}),
            deferred: false,
            risk,
            concurrency: Concurrency::Parallel,
        }
    }

    fn output(text: &str) -> ToolOutput {
        ToolOutput {
            text: text.into(),
            is_error: false,
            diff: None,
            structured: None,
        }
    }

    fn path_of(input: &Value) -> String {
        input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    struct StubRead(Shared);
    struct StubBash(Shared);
    struct StubWrite(Shared);

    #[async_trait::async_trait]
    impl Tool for StubRead {
        fn spec(&self) -> ToolSpec {
            spec("read", Risk::ReadOnly)
        }
        fn subject(&self, input: &Value) -> String {
            path_of(input)
        }
        async fn call(&self, input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .files
                .get(&path_of(&input))
                .cloned()
                .map(|text| output(&text))
                .ok_or(ToolError::NotFound)
        }
    }

    #[async_trait::async_trait]
    impl Tool for StubBash {
        fn spec(&self) -> ToolSpec {
            spec("bash", Risk::ReadOnly)
        }
        fn subject(&self, input: &Value) -> String {
            input
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        }
        async fn call(&self, _input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
            Ok(output(
                &self.0.lock().unwrap_or_else(|e| e.into_inner()).listing,
            ))
        }
    }

    #[async_trait::async_trait]
    impl Tool for StubWrite {
        fn spec(&self) -> ToolSpec {
            spec("write", Risk::Write)
        }
        fn subject(&self, input: &Value) -> String {
            path_of(input)
        }
        async fn call(&self, input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
            let content = input
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .writes
                .push((path_of(&input), content.clone()));
            Ok(output(&format!("wrote {} bytes", content.len())))
        }
    }

    fn session(fs: Shared, scenario: &str) -> (Session, Arc<MemoryStore>, mpsc::Receiver<Event>) {
        let mut config = cox_protocol::Config::default();
        let cwd = PathBuf::from("/tmp/cox-init");
        config.core.workspace_roots = vec![cwd.clone()];
        let provider = Arc::new(Scripted::from_toml(scenario, "").expect("scenario"));
        let store = Arc::new(MemoryStore::new());
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(StubRead(fs.clone())),
            Arc::new(StubBash(fs.clone())),
            Arc::new(StubWrite(fs)),
        ];
        let session = Session::new(config, provider, tools, store.clone(), store.clone(), cwd)
            .expect("session");
        let rx = session.events().expect("events once");
        (session, store, rx)
    }

    /// Runs `run_init` to completion, allowing the one `write` approval the
    /// permission engine raises for it.
    async fn init_with_approval(session: &Session, rx: &mut mpsc::Receiver<Event>, force: bool) {
        let running = {
            let session = session.clone();
            tokio::spawn(async move { session.run_init(force).await })
        };
        loop {
            let ev = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
                .await
                .expect("approval timeout")
                .expect("event stream closed");
            if let Event::ApprovalRequired { call, .. } = ev {
                session
                    .submit(Submission::Approve {
                        call_id: call.id,
                        decision: Decision::Allow,
                    })
                    .await
                    .expect("approve");
                break;
            }
        }
        running.await.expect("join").expect("init");
    }

    fn fs_with_readme() -> Shared {
        Arc::new(StdMutex::new(Fs {
            listing: "Cargo.toml\nREADME.md\njustfile\nsrc/\ntests/\n[exit 0 in 2ms]\n".into(),
            files: HashMap::from([("README.md".into(), "# Sample\nBuild with cargo.".into())]),
            writes: Vec::new(),
        }))
    }

    /// T25.6: the `ls -1p` parser survives the `bash` tool's PTY, where
    /// entries share tab-separated lines, and skips the `[exit …]` trailer.
    #[test]
    fn init_parse_ls_handles_pty_columns_and_the_exit_trailer() {
        let parsed = parse_ls("Cargo.toml\tREADME.md\tjustfile\nsrc/\ttests/\n[exit 0 in 2ms]\n");
        let names: Vec<&str> = parsed.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            ["Cargo.toml", "README.md", "justfile", "src", "tests"]
        );
        assert!(parsed.iter().any(|(name, dir)| name == "src" && *dir));
        assert!(parse_ls("").is_empty());
        assert!(parse_ls("[exit 1 in 5ms]\n").is_empty());
    }

    /// T25.6: the listing's manifests become commands, the cheap summary
    /// becomes conventions, and the write waits for approval first.
    #[tokio::test]
    async fn init_detects_cargo_and_just() {
        let fs = fs_with_readme();
        let (session, store, mut rx) =
            session(fs.clone(), "[[turn]]\ntext = \"- Run cargo test.\"\n");
        init_with_approval(&session, &mut rx, false).await;
        let fs = fs.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(fs.writes.len(), 1, "one approved write");
        let (path, content) = &fs.writes[0];
        assert_eq!(path, "AGENTS.md");
        assert!(content.contains("cargo build"), "{content}");
        assert!(content.contains("just test"), "{content}");
        assert!(content.contains("- Run cargo test."), "{content}");
        assert!(
            store
                .usage_rows()
                .iter()
                .any(|row| row.job == Job::Summarize && row.tier == Tier::Cheap),
            "the README summary is a cheap-tier ledger row"
        );
    }

    /// T25.6: an existing `AGENTS.md` without `--force` is a warning, never
    /// a write; with it the flow runs.
    #[tokio::test]
    async fn init_refuses_to_overwrite() {
        let fs: Shared = Arc::new(StdMutex::new(Fs {
            listing: "AGENTS.md\nREADME.md\n[exit 0 in 2ms]\n".into(),
            files: HashMap::from([
                ("AGENTS.md".into(), "keep me".into()),
                ("README.md".into(), "hi".into()),
            ]),
            writes: Vec::new(),
        }));
        let (session, _, mut rx) = session(fs.clone(), "");
        session
            .run_init(false)
            .await
            .expect("refusal is not an error");
        let mut warned = false;
        // The refusal path emits the read/ls request and done events first;
        // wait for the warning however far down the stream it is.
        for _ in 0..10 {
            let ev = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
                .await
                .expect("event timeout")
                .expect("event stream closed");
            if matches!(&ev, Event::Notice { level: Level::Warn, text } if text.contains("already exists"))
            {
                warned = true;
                break;
            }
        }
        assert!(warned, "the refusal names the existing file");
        assert!(
            fs.lock()
                .unwrap_or_else(|e| e.into_inner())
                .writes
                .is_empty(),
            "nothing is written without --force"
        );
        init_with_approval(&session, &mut rx, true).await;
        assert_eq!(
            fs.lock().unwrap_or_else(|e| e.into_inner()).writes.len(),
            1,
            "--force runs the same flow"
        );
    }
}
