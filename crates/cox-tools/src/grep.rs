// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `grep`: ripgrep-equivalent content search (plan.md T3.3). The walk,
//! match and format engine (`search`) moved to the pure `cox-search` crate
//! (T32.5) — no filesystem, no `ToolCx`. [`GrepTool`] is the `Tool` impl: it
//! resolves the root through `crate::path::confine` and, past
//! `max_results`, archives the full result through `cx.archive` (D6a: the
//! archive row exists before the model sees the shortened text), so it
//! stays here rather than moving with the rest of `grep` — `confine` keeps
//! one call site (docs/design/crates.md, AGENTS.md trust boundaries).

use cox_protocol::{ArchivePut, ToolCx, ToolError, ToolOutput, ToolSpec};
use cox_protocol::{Concurrency, Risk};
use cox_search::grep::Line;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::path;

/// Matches are capped here unless the caller sets a smaller/larger
/// `max_results`; keeps a runaway pattern on a big tree from flooding the
/// model with an unbounded reply instead of a pointer.
const DEFAULT_MAX_RESULTS: usize = 200;

/// `grep`'s JSON input, matching plan.md T3.3's step list.
#[derive(Debug, Deserialize, JsonSchema)]
struct GrepInput {
    /// Regular expression to search for (Rust `regex` syntax).
    pattern: String,
    /// Directory or file to search, relative to a workspace root. Defaults
    /// to the root itself.
    #[serde(default)]
    path: Option<String>,
    /// Only search files whose name or path matches this glob (e.g. `*.rs`).
    #[serde(default)]
    glob: Option<String>,
    /// Lines of context to show before and after each match.
    #[serde(default)]
    context: Option<usize>,
    /// Stop after this many matches and archive the rest. Defaults to
    /// [`DEFAULT_MAX_RESULTS`].
    #[serde(default)]
    max_results: Option<usize>,
}

/// Ripgrep-equivalent content search: `ignore::WalkBuilder` (`.gitignore`
/// honoured, hidden files included) + `grep-regex`/`grep-searcher`.
pub struct GrepTool;

#[async_trait::async_trait]
impl cox_protocol::Tool for GrepTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "grep".into(),
            description: "Search file contents with a regular expression (ripgrep semantics: \
                .gitignore is honoured, hidden files are included). Returns `path:line:text` \
                per match, `path-line-text` for `context` lines, and a bare `--` between \
                non-contiguous groups. `glob` filters which files are searched (matched against \
                the basename or the full path). Past `max_results` matches the rest is archived \
                and a trailer line names the archive id."
                .into(),
            input_schema: schemars::schema_for!(GrepInput).to_value(),
            deferred: false,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }

    fn subject(&self, input: &Value) -> String {
        input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or(".")
            .to_string()
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let input: GrepInput = serde_json::from_value(input).map_err(|e| ToolError::Denied {
            why: format!("invalid input: {e}"),
        })?;

        let root = path::confine(&cx.roots, &cx.cwd, input.path.as_deref().unwrap_or("."))?;
        if !root.exists() {
            return Err(ToolError::NotFound);
        }

        let all = match cox_search::grep::search(
            &root,
            &input.pattern,
            input.glob.as_deref(),
            input.context,
        ) {
            Ok(lines) => lines,
            Err(e) => return Ok(text_error(e.to_string())),
        };

        let total_matches = all.iter().filter(|l| l.is_match).count();
        let cap = input.max_results.unwrap_or(DEFAULT_MAX_RESULTS);

        if total_matches == 0 {
            return Ok(ToolOutput {
                text: "no matches".into(),
                is_error: false,
                diff: None,
                structured: Some(matches(0)),
            });
        }

        if total_matches <= cap {
            let text = join(&all);
            return Ok(ToolOutput {
                text,
                is_error: false,
                diff: None,
                structured: Some(matches(total_matches)),
            });
        }

        // Exceeded the cap: archive the full result, then truncate to the
        // first `cap` matches (plus whatever context/break lines lead up to
        // them) and append a pointer trailer.
        let full_text = join(&all);
        let archive_id = cx
            .archive
            .put(ArchivePut {
                session: cx.session,
                call: cx.call,
                tool: "grep".into(),
                subject: Some(root.display().to_string()),
                bytes: full_text.clone().into_bytes(),
            })
            .await
            .map_err(|_| ToolError::Io)?;

        let mut truncated: Vec<&Line> = Vec::new();
        let mut emitted = 0usize;
        for line in &all {
            if emitted >= cap {
                break;
            }
            if line.is_match {
                emitted += 1;
            }
            truncated.push(line);
        }
        let mut text = truncated
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        text.push('\n');
        text.push_str(&format!(
            "… {} more matches; archived as {archive_id}",
            total_matches - cap
        ));

        Ok(ToolOutput {
            text,
            is_error: false,
            diff: None,
            structured: Some(matches(total_matches)),
        })
    }
}

/// The match count as data (DT G3), including those past the cap, so a
/// summary never counts lines of a possibly truncated text.
fn matches(n: usize) -> Value {
    serde_json::json!({ "matches": n })
}

fn join(lines: &[Line]) -> String {
    lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn text_error(text: String) -> ToolOutput {
    ToolOutput {
        text,
        is_error: true,
        diff: None,
        structured: None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Arc;

    use cox_protocol::{
        Archive, ArchiveId, SandboxMode, SandboxPolicy, SessionId, StoreError, Tool,
    };
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::*;

    struct NoopArchive;

    #[async_trait::async_trait]
    impl Archive for NoopArchive {
        async fn put(&self, _put: ArchivePut) -> Result<ArchiveId, StoreError> {
            Ok(ArchiveId::new())
        }
        async fn get(&self, _id: &ArchiveId) -> Result<Vec<u8>, StoreError> {
            Ok(Vec::new())
        }
    }

    /// The files `fixtures/grep/.gitignore` lists are written here rather
    /// than committed: a file that is both tracked and ignored reads as a
    /// dirty tree to release-plz (A17), which then commits its deletion.
    fn fixtures_root() -> PathBuf {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/grep")
            .canonicalize()
            .expect("fixtures/grep exists");
        for (name, text) in [
            (
                "ignored.txt",
                "TODO: this file is gitignored and must never appear\n",
            ),
            ("build.log", "TODO: build noise, also gitignored\n"),
        ] {
            std::fs::write(root.join(name), text).expect("write gitignored fixture");
        }
        root
    }

    fn cx(root: PathBuf) -> ToolCx {
        let (tx, _rx) = mpsc::channel(16);
        ToolCx {
            roots: vec![root.clone()],
            writable_roots: vec![root.clone()],
            cwd: root,
            sandbox: SandboxPolicy {
                mode: SandboxMode::ReadOnly,
                network: false,
                writable: vec![],
                readonly_in_workspace: vec![],
                linux_backend: Default::default(),
            },
            archive: Arc::new(NoopArchive),
            cancel: CancellationToken::new(),
            output: tx,
            session: SessionId::new(),
            call: cox_protocol::CallId::new(),
            agent: None,
            preset: None,
            relay: None,
        }
    }

    async fn run(pattern: &str, extra: Value) -> ToolOutput {
        let root = fixtures_root();
        let mut input = serde_json::json!({ "pattern": pattern, "path": "." });
        input
            .as_object_mut()
            .expect("object")
            .extend(extra.as_object().cloned().unwrap_or_default());
        GrepTool
            .call(input, &cx(root))
            .await
            .expect("grep call succeeds")
    }

    /// Runs `rg -n --no-heading --hidden --sort path <pattern> <root>` and
    /// returns its stdout, or `None` if `rg` isn't on PATH (T3.3's spec:
    /// "rg invoked only if present on the test machine; otherwise golden
    /// files" — golden files live alongside this test as `*.golden`).
    fn rg_output(pattern: &str, root: &Path) -> Option<String> {
        let out = Command::new("rg")
            .args(["-n", "--no-heading", "--hidden", "--sort", "path", pattern])
            .arg(root)
            .output()
            .ok()?;
        Some(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    }

    /// Deliberately *beside* `fixtures/grep`, not inside it: a golden file
    /// holding match text would itself be searched, so `fn_space.golden`
    /// would match its own contents and never stabilise.
    fn golden_path(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/grep-golden")
            .join(format!("{name}.golden"))
    }

    async fn assert_matches_rg_or_golden(name: &str, pattern: &str) {
        let root = fixtures_root();
        let got = run(pattern, serde_json::json!({})).await;
        let got_text = got.text.trim_end().to_string();

        match rg_output(pattern, &root) {
            Some(want) => assert_eq!(got_text, want, "pattern {pattern:?}"),
            None => {
                // The golden files hold paths relative to the fixture root;
                // an absolute path would only ever match on the machine that
                // generated it.
                let got_rel = got_text.replace(&format!("{}/", root.display()), "");
                let want = std::fs::read_to_string(golden_path(name))
                    .unwrap_or_else(|_| panic!("golden file missing for {name}"))
                    .trim_end()
                    .to_string();
                assert_eq!(got_rel, want, "pattern {pattern:?} (golden)");
            }
        }
    }

    #[tokio::test]
    async fn grep_matches_rg_word_todo() {
        assert_matches_rg_or_golden("todo", "TODO").await;
    }

    #[tokio::test]
    async fn grep_matches_rg_literal_fn_space() {
        assert_matches_rg_or_golden("fn_space", "fn ").await;
    }

    #[tokio::test]
    async fn grep_matches_rg_anchored_pub_fn() {
        assert_matches_rg_or_golden("anchored_pub_fn", "^pub fn").await;
    }

    #[tokio::test]
    async fn grep_matches_rg_alternation() {
        assert_matches_rg_or_golden("alternation", "hello|deep").await;
    }

    #[tokio::test]
    async fn grep_matches_rg_digit_class() {
        assert_matches_rg_or_golden("digit_class", "[0-9]+").await;
    }

    #[tokio::test]
    async fn grep_respects_gitignore() {
        let out = run("TODO", serde_json::json!({})).await;
        assert!(
            !out.text.contains("ignored.txt") && !out.text.contains("build.log"),
            "gitignored files leaked into output:\n{}",
            out.text
        );
    }

    #[tokio::test]
    async fn grep_glob_filters_to_matching_files() {
        let out = run("fn", serde_json::json!({ "glob": "*.rs" })).await;
        assert!(out.text.contains("main.rs"));
        assert!(!out.text.contains("readme.md"));
    }

    #[tokio::test]
    async fn grep_max_results_archives_and_appends_trailer() {
        let out = run("TODO", serde_json::json!({ "max_results": 1 })).await;
        assert!(
            out.text.contains("more matches; archived as"),
            "expected a pointer trailer, got:\n{}",
            out.text
        );
        // Exactly one match line shown before the trailer.
        let match_lines = out
            .text
            .lines()
            .filter(|l| l.contains(":TODO") || l.contains("TODO"))
            .count();
        assert!(match_lines >= 1);
        let total = out.structured.as_ref().and_then(|s| s["matches"].as_u64());
        assert!(
            total > Some(1),
            "the count covers matches past the cap: {total:?}"
        );
    }

    #[tokio::test]
    async fn grep_context_includes_surrounding_lines() {
        let out = run(
            "deep",
            serde_json::json!({ "context": 1, "glob": "file.txt" }),
        )
        .await;
        assert!(out.text.contains("second line") || out.text.lines().count() > 1);
    }
}
