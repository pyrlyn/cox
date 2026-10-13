// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The `definition` and `references` tools (T59.6): where a symbol is
//! defined and where it is used, from a language server that already runs
//! for the file, else from a word-boundary `grep` answer marked
//! `(no server)`. Separate from `mod.rs` because the wire types
//! (`Location`, `LocationLink`), the position arithmetic and the text format
//! are pure and tested without a pipe. A navigation call never starts a
//! server: only `diagnostics` does, since starting one runs the project's
//! build scripts and needs the permission ask that tool carries.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use cox_protocol::{ArchivePut, Concurrency, Risk, Tool, ToolCx, ToolError, ToolOutput, ToolSpec};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::{Value, json};

use super::client::LspError;
use super::diag::{Position, Range, path_for};
use super::{LspPool, load, root_for};
use crate::path::confine;

/// Locations listed in one result; the rest is archived (D6a).
const MAX_LOCATIONS: usize = 100;
/// Longest source line quoted next to a location, in characters.
const MAX_SNIPPET: usize = 160;

/// Which request a tool sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    Definition,
    References,
}

impl Nav {
    pub fn method(self) -> &'static str {
        match self {
            Self::Definition => "textDocument/definition",
            Self::References => "textDocument/references",
        }
    }

    pub fn params(self, uri: &str, at: Position) -> Value {
        let mut params = json!({
            "textDocument": {"uri": uri},
            "position": {"line": at.line, "character": at.character},
        });
        if self == Self::References {
            // The declaration is a use the model asked about too.
            params["context"] = json!({"includeDeclaration": true});
        }
        params
    }

    fn name(self) -> &'static str {
        match self {
            Self::Definition => "definition",
            Self::References => "references",
        }
    }
}

/// A place in a file as the server names it: `Location`, or the target of a
/// `LocationLink`.
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

#[derive(Deserialize)]
struct RawLocation {
    uri: String,
    range: Range,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawLink {
    target_uri: String,
    target_range: Range,
    target_selection_range: Option<Range>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawItem {
    Location(RawLocation),
    Link(RawLink),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawReply {
    Many(Vec<RawItem>),
    One(RawLocation),
}

impl From<RawItem> for Location {
    fn from(item: RawItem) -> Self {
        match item {
            RawItem::Location(l) => Self {
                uri: l.uri,
                range: l.range,
            },
            // The selection range is the name, not the whole body.
            RawItem::Link(l) => Self {
                uri: l.target_uri,
                range: l.target_selection_range.unwrap_or(l.target_range),
            },
        }
    }
}

/// The result of `definition` or `references`: `null`, one `Location`, or
/// a list of `Location`s or `LocationLink`s.
pub fn parse_locations(reply: Value) -> Result<Vec<Location>, LspError> {
    if reply.is_null() {
        return Ok(Vec::new());
    }
    let raw: RawReply = serde_json::from_value(reply)
        .map_err(|e| LspError::Parse(format!("location reply: {e}")))?;
    Ok(match raw {
        RawReply::Many(items) => items.into_iter().map(Location::from).collect(),
        RawReply::One(l) => vec![Location {
            uri: l.uri,
            range: l.range,
        }],
    })
}

/// What a `definition` or `references` call is asked about.
#[derive(Debug, Deserialize, JsonSchema)]
struct NavInput {
    /// File holding the symbol, relative to the session `cwd` or absolute within a workspace root.
    path: String,
    /// 1-based line of the symbol.
    line: u32,
    /// 1-based column (in characters) of any character of the symbol.
    col: u32,
}

/// `definition` or `references`, by [`Nav`]; shares the pool of
/// `diagnostics`, so it sees the servers that tool started.
pub struct NavTool {
    pool: Arc<LspPool>,
    nav: Nav,
    /// The terminal-text guard (`cox_sanitize::sanitize`), handed in because
    /// `cox-tools` may not depend on it: a server's paths are untrusted.
    sanitize: fn(&str) -> String,
}

impl NavTool {
    pub fn new(pool: Arc<LspPool>, nav: Nav, sanitize: fn(&str) -> String) -> Self {
        Self {
            pool,
            nav,
            sanitize,
        }
    }
}

#[async_trait]
impl Tool for NavTool {
    fn spec(&self) -> ToolSpec {
        let (what, shape) = match self.nav {
            Nav::Definition => (
                "Finds where the symbol at a position is defined",
                "`path:line:col: source line` per definition",
            ),
            Nav::References => (
                "Finds every use of the symbol at a position, its declaration included",
                "`path:line:col: source line` per use, sorted by path",
            ),
        };
        ToolSpec {
            name: self.nav.name().to_string(),
            description: format!(
                "{what}, as {shape}; past {MAX_LOCATIONS} the rest is archived. `line` and \
                 `col` are 1-based and point into the file as `read` shows it. Answers from \
                 the file's language server when one already runs (call `diagnostics` on a \
                 file of that language to start it); this tool never starts one. Without a \
                 server it falls back to a word-boundary `grep` of the identifier under the \
                 position and says `(no server)`, which also matches unrelated symbols of \
                 that name."
            ),
            input_schema: serde_json::to_value(schema_for!(NavInput)).unwrap_or(Value::Null),
            deferred: true,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }

    fn subject(&self, input: &Value) -> String {
        input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let input: NavInput = serde_json::from_value(input).map_err(|e| ToolError::Denied {
            why: format!("invalid input: {e}"),
        })?;
        let path = confine(&cx.roots, &cx.cwd, &input.path)?;
        let text = load(&path)?;
        let at = position(&text, input.line, input.col)?;
        let root = root_for(&path, &cx.roots, &cx.cwd);

        let why_not = match self.pool.running_for(&path) {
            None => "no server".to_string(),
            Some(server) => {
                let answer = tokio::select! {
                    r = server.locations(self.nav, &path, &text, at) => r,
                    () = cx.cancel.cancelled() => return Err(ToolError::Cancelled),
                };
                match answer {
                    Ok(found) => {
                        let lines = list(&found, cx, &root);
                        let empty = format!("no {} found", self.nav.name());
                        let out = capped(self.nav, &path, lines, empty, cx).await?;
                        return Ok(self.reply((self.sanitize)(&out)));
                    }
                    // A server that fails or is still loading is not an
                    // error for the model: the grep answer is still useful.
                    Err(e) => format!("no server: it failed: {e}"),
                }
            }
        };
        let out = self.fallback(&path, &text, input, &why_not, cx).await?;
        Ok(self.reply((self.sanitize)(&out)))
    }
}

impl NavTool {
    fn reply(&self, text: String) -> ToolOutput {
        ToolOutput {
            text,
            is_error: false,
            diff: None,
            structured: None,
        }
    }

    /// A word-boundary `grep` of the identifier at the position, over the
    /// session's roots.
    async fn fallback(
        &self,
        path: &Path,
        text: &str,
        input: NavInput,
        why_not: &str,
        cx: &ToolCx,
    ) -> Result<String, ToolError> {
        let Some(word) = word_at(text, input.line, input.col) else {
            return Ok(format!(
                "no identifier at {}:{}:{} ({why_not})",
                input.path, input.line, input.col
            ));
        };
        let pattern = fallback_pattern(self.nav, &word);
        let search_root = confine(&cx.roots, &cx.cwd, ".")?;
        let found = cox_search::grep::search(&search_root, &pattern, None, None)
            .map_err(|_| ToolError::Io)?;
        let lines: Vec<String> = found
            .iter()
            .filter(|l| l.is_match)
            .map(|l| {
                let text = l.text.strip_prefix(&format!("{}/", search_root.display()));
                clip(text.unwrap_or(&l.text))
            })
            .collect();
        let empty = format!("no {} found for `{word}`", self.nav.name());
        let mut out = capped(self.nav, path, lines, empty, cx).await?;
        out.push_str(&format!(
            "\n({why_not}: grep for `{word}`, so this may include unrelated symbols of that name)"
        ));
        Ok(out)
    }
}

/// The 0-based [`Position`] of 1-based `line` and character `col` in
/// `text`, the column converted to the UTF-16 units servers count in.
fn position(text: &str, line: u32, col: u32) -> Result<Position, ToolError> {
    let denied = |why: String| ToolError::Denied { why };
    if line == 0 || col == 0 {
        return Err(denied("line and col are 1-based".to_string()));
    }
    let Some(row) = text.lines().nth(line as usize - 1) else {
        return Err(denied(format!("the file has no line {line}")));
    };
    let before = col as usize - 1;
    if before > row.chars().count() {
        return Err(denied(format!("line {line} has no column {col}")));
    }
    let character = row.chars().take(before).map(char::len_utf16).sum::<usize>();
    Ok(Position {
        line: line - 1,
        character: u32::try_from(character).unwrap_or(u32::MAX),
    })
}

/// The identifier (letters, digits, `_`) the 1-based position touches; a
/// position just past its last character counts, as an editor cursor does.
fn word_at(text: &str, line: u32, col: u32) -> Option<String> {
    let row: Vec<char> = text.lines().nth(line as usize - 1)?.chars().collect();
    let word = |c: &char| c.is_alphanumeric() || *c == '_';
    let at = (col as usize).checked_sub(1)?;
    let start_from = if row.get(at).is_some_and(word) {
        at
    } else if at > 0 && row.get(at - 1).is_some_and(word) {
        at - 1
    } else {
        return None;
    };
    let start = row[..=start_from]
        .iter()
        .rposition(|c| !word(c))
        .map_or(0, |i| i + 1);
    let end = row[start_from..]
        .iter()
        .position(|c| !word(c))
        .map_or(row.len(), |i| start_from + i);
    Some(row[start..end].iter().collect())
}

/// The regex for the fallback: every whole-word use, or for `definition`
/// only the lines a common language introduces a name on. `word` holds only
/// word characters, so it needs no escaping.
fn fallback_pattern(nav: Nav, word: &str) -> String {
    match nav {
        Nav::References => format!(r"\b{word}\b"),
        Nav::Definition => format!(
            r"\b(fn|struct|enum|trait|type|const|static|mod|union|def|class|function|func|interface|let|var|val|object)\s+(mut\s+)?{word}\b"
        ),
    }
}

/// One `path:line:col: source line` per location, sorted and deduplicated.
/// A path the server names outside the workspace roots is printed bare: it
/// is untrusted, so it is never opened (`confine` is the one gate).
fn list(found: &[Location], cx: &ToolCx, root: &Path) -> Vec<String> {
    let mut files: HashMap<PathBuf, Option<Vec<String>>> = HashMap::new();
    let mut rows: BTreeSet<(String, u32, u32)> = BTreeSet::new();
    let mut shown: Vec<(String, u32, u32, Option<String>)> = Vec::new();
    for loc in found {
        let Some(file) = path_for(&loc.uri) else {
            continue;
        };
        let start = loc.range.start;
        let confined = confine(&cx.roots, &cx.cwd, &file.to_string_lossy()).ok();
        let name = confined.as_ref().map_or(file.display().to_string(), |p| {
            p.strip_prefix(root).unwrap_or(p).display().to_string()
        });
        if !rows.insert((name.clone(), start.line, start.character)) {
            continue;
        }
        let snippet = confined.and_then(|p| {
            let lines = files.entry(p.clone()).or_insert_with(|| {
                load(&p)
                    .ok()
                    .map(|t| t.lines().map(str::to_string).collect())
            });
            let line = lines.as_ref()?.get(start.line as usize)?;
            Some(clip(line.trim()))
        });
        shown.push((name, start.line, start.character, snippet));
    }
    shown.sort_by(|a, b| (&a.0, a.1, a.2).cmp(&(&b.0, b.1, b.2)));
    shown
        .into_iter()
        .map(|(name, line, col, snippet)| match snippet {
            Some(s) => format!("{name}:{}:{}: {s}", line + 1, col + 1),
            None => format!("{name}:{}:{}", line + 1, col + 1),
        })
        .collect()
}

fn clip(line: &str) -> String {
    match line.char_indices().nth(MAX_SNIPPET) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line.to_string(),
    }
}

/// The lines as text, the first [`MAX_LOCATIONS`] of them: the whole list is
/// archived first, so the shortened text always names a row that exists.
async fn capped(
    nav: Nav,
    path: &Path,
    lines: Vec<String>,
    empty: String,
    cx: &ToolCx,
) -> Result<String, ToolError> {
    if lines.is_empty() {
        return Ok(empty);
    }
    if lines.len() <= MAX_LOCATIONS {
        return Ok(lines.join("\n"));
    }
    let id = cx
        .archive
        .put(ArchivePut {
            session: cx.session,
            call: cx.call,
            tool: nav.name().into(),
            subject: Some(path.display().to_string()),
            bytes: lines.join("\n").into_bytes(),
        })
        .await
        .map_err(|_| ToolError::Io)?;
    let more = lines.len() - MAX_LOCATIONS;
    let mut out = lines[..MAX_LOCATIONS].join("\n");
    out.push_str(&format!("\n… {more} more; archived as {id}"));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{Started, cx, tool, workspace};
    use super::super::{DiagnosticsTool, lock};
    use super::*;

    /// Stands in for `cox_sanitize::sanitize`, which `cox-tools` may not
    /// depend on; the session hands in the real one.
    fn strip_escapes(s: &str) -> String {
        s.replace('\u{1b}', "")
    }

    fn nav_tool(diags: &DiagnosticsTool, nav: Nav) -> NavTool {
        NavTool::new(diags.pool.clone(), nav, strip_escapes)
    }

    fn at(path: &str, line: u32, col: u32) -> Value {
        json!({"path": path, "line": line, "col": col})
    }

    /// A workspace with `src/a.rs` holding `a` and a server that runs.
    async fn with_server(a: &str) -> (tempfile::TempDir, PathBuf, DiagnosticsTool, Started) {
        let (dir, root) = workspace(&["src/a.rs"]);
        std::fs::write(root.join("src/a.rs"), a).unwrap();
        let (diags, started) = tool(&[("rust", "/bin/sh", "rs")]);
        let out = diags
            .call(json!({"path": "src/a.rs"}), &cx(&root))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.text);
        (dir, root, diags, started)
    }

    #[tokio::test(start_paused = true)]
    async fn definition_asks_the_running_server() {
        let (_dir, root, diags, started) = with_server("fn main() {}\n").await;
        let def = nav_tool(&diags, Nav::Definition);

        let out = def.call(at("src/a.rs", 1, 5), &cx(&root)).await.unwrap();
        assert!(!out.is_error, "{}", out.text);
        // The fake echoes the position it was sent: line 0, character 4.
        assert_eq!(out.text, "src/a.rs:1:5: fn main() {}");
        assert_eq!(lock(&started).len(), 1, "no second server");
    }

    #[tokio::test(start_paused = true)]
    async fn a_column_counts_characters_and_the_server_gets_utf16() {
        let (_dir, root, diags, _) = with_server("fn 𝒳y() {}\n").await;
        let def = nav_tool(&diags, Nav::Definition);

        // `(` is the 6th character and sits at UTF-16 unit 6, as `𝒳` is two.
        let out = def.call(at("src/a.rs", 1, 6), &cx(&root)).await.unwrap();
        assert!(out.text.starts_with("src/a.rs:1:7: "), "{}", out.text);
    }

    #[tokio::test(start_paused = true)]
    async fn references_are_sorted_deduplicated_and_outside_paths_stay_bare() {
        let (_dir, root, diags, _) = with_server("fn main() {}\n\nlet x = 1;\n").await;
        let refs = nav_tool(&diags, Nav::References);

        let out = refs.call(at("src/a.rs", 1, 4), &cx(&root)).await.unwrap();
        // The server's path is not read, and its escape sequence is gone.
        assert_eq!(
            out.text,
            "/elsewhere/[31mx.rs:5:2\nsrc/a.rs:1:4: fn main() {}\nsrc/a.rs:3:1: let x = 1;"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn references_past_the_cap_are_archived() {
        let (_dir, root, diags, _) = with_server("MANY\n").await;
        let refs = nav_tool(&diags, Nav::References);

        let out = refs.call(at("src/a.rs", 1, 1), &cx(&root)).await.unwrap();
        assert_eq!(out.text.lines().count(), MAX_LOCATIONS + 1, "{}", out.text);
        assert!(
            out.text.contains("\n… 50 more; archived as "),
            "{}",
            out.text
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_empty_answer_is_an_answer_not_a_fallback() {
        let (_dir, root, diags, _) = with_server("NOWHERE\n").await;
        let def = nav_tool(&diags, Nav::Definition);

        let out = def.call(at("src/a.rs", 1, 2), &cx(&root)).await.unwrap();
        assert_eq!(out.text, "no definition found");
    }

    #[tokio::test(start_paused = true)]
    async fn without_a_running_server_references_fall_back_to_grep() {
        let (_dir, root) = workspace(&["src/a.rs", "src/b.rs"]);
        std::fs::write(root.join("src/b.rs"), "fn other() { main(); }\n").unwrap();
        let (diags, started) = tool(&[("rust", "/bin/sh", "rs")]);
        let refs = nav_tool(&diags, Nav::References);

        let out = refs.call(at("src/a.rs", 1, 5), &cx(&root)).await.unwrap();
        assert!(!out.is_error, "{}", out.text);
        let mut lines: Vec<&str> = out.text.lines().collect();
        let note = lines.pop().unwrap();
        lines.sort_unstable();
        assert_eq!(
            lines,
            [
                "src/a.rs:1:fn main() {}",
                "src/b.rs:1:fn other() { main(); }"
            ]
        );
        assert!(note.starts_with("(no server: grep for `main`"), "{note}");
        assert!(
            lock(&started).is_empty(),
            "navigation never starts a server"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_definition_fallback_only_matches_declarations() {
        let (_dir, root) = workspace(&["src/a.rs", "src/b.rs"]);
        std::fs::write(root.join("src/b.rs"), "fn other() { main(); }\n").unwrap();
        let (diags, _) = tool(&[("rust", "/bin/sh", "rs")]);
        let def = nav_tool(&diags, Nav::Definition);

        // Asked from the call site, where `main` ends the identifier.
        let out = def.call(at("src/b.rs", 1, 16), &cx(&root)).await.unwrap();
        assert!(
            out.text.starts_with("src/a.rs:1:fn main() {}\n(no server"),
            "{}",
            out.text
        );
        assert!(!out.text.contains("src/b.rs:"), "{}", out.text);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failing_server_falls_back_and_says_why() {
        let (_dir, root, diags, _) = with_server("FAIL main\n").await;
        let def = nav_tool(&diags, Nav::Definition);

        let out = def.call(at("src/a.rs", 1, 7), &cx(&root)).await.unwrap();
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("(no server: it failed: "), "{}", out.text);
        assert!(out.text.contains("boom"), "{}", out.text);
    }

    #[tokio::test(start_paused = true)]
    async fn bad_input_is_refused_before_anything_runs() {
        let (_dir, root) = workspace(&["src/a.rs"]);
        let (diags, started) = tool(&[("rust", "/bin/sh", "rs")]);
        let def = nav_tool(&diags, Nav::Definition);

        let outside = def.call(at("../../elsewhere/b.rs", 1, 1), &cx(&root)).await;
        assert!(
            matches!(outside, Err(ToolError::Confined { .. })),
            "{outside:?}"
        );
        for (line, col) in [(0, 1), (1, 0), (2, 1), (1, 99)] {
            let out = def.call(at("src/a.rs", line, col), &cx(&root)).await;
            assert!(
                matches!(out, Err(ToolError::Denied { .. })),
                "{line}:{col} {out:?}"
            );
        }
        let blank = def.call(at("src/a.rs", 1, 9), &cx(&root)).await.unwrap();
        assert!(
            blank.text.starts_with("no identifier at "),
            "{}",
            blank.text
        );
        assert!(lock(&started).is_empty());
    }

    #[test]
    fn locations_parse_in_every_shape_the_spec_allows() {
        let p = json!({"line": 1, "character": 2});
        let range = json!({"start": p, "end": p});
        let one = parse_locations(json!({"uri": "file:///a.rs", "range": range})).unwrap();
        let many = parse_locations(json!([{"uri": "file:///a.rs", "range": range}])).unwrap();
        assert_eq!(one, many);
        assert_eq!(
            one[0].range.start,
            Position {
                line: 1,
                character: 2
            }
        );

        let link = json!([{
            "targetUri": "file:///b.rs",
            "targetRange": {"start": {"line": 0, "character": 0}, "end": p},
            "targetSelectionRange": range,
        }]);
        let link = parse_locations(link).unwrap();
        assert_eq!(link[0].uri, "file:///b.rs");
        assert_eq!(
            link[0].range.start,
            Position {
                line: 1,
                character: 2
            }
        );

        assert!(parse_locations(Value::Null).unwrap().is_empty());
        assert!(parse_locations(json!([])).unwrap().is_empty());
        assert!(matches!(
            parse_locations(json!("x")),
            Err(LspError::Parse(_))
        ));
    }

    #[test]
    fn word_at_takes_the_identifier_under_or_just_before_the_cursor() {
        let text = "let foo_1 = bar;\n";
        assert_eq!(word_at(text, 1, 6).as_deref(), Some("foo_1"));
        assert_eq!(word_at(text, 1, 10).as_deref(), Some("foo_1"));
        assert_eq!(word_at(text, 1, 11), None);
        assert_eq!(word_at(text, 1, 1).as_deref(), Some("let"));
    }

    #[test]
    fn spec_is_deferred_and_read_only() {
        let (diags, _) = tool(&[("rust", "/bin/sh", "rs")]);
        for (nav, name) in [
            (Nav::Definition, "definition"),
            (Nav::References, "references"),
        ] {
            let spec = nav_tool(&diags, nav).spec();
            assert_eq!(spec.name, name);
            assert!(spec.deferred);
            assert_eq!(spec.risk, Risk::ReadOnly);
        }
    }
}
