// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `rename_symbol` (T59.7): renames the symbol at a position across files
//! with the `WorkspaceEdit` of the language server that already runs for
//! the file. Separate from `nav.rs` because it writes: the server's edit is
//! untrusted, so every file it names goes through `confine` against the
//! writable roots, a versioned edit made against text cox no longer holds
//! is refused, and the whole set is written by `write::rewrite_all` (all
//! or nothing, each pre-image archived) only after every file's new text
//! is computed. There is no text-search fallback: a rename by name match
//! would rewrite unrelated symbols, so without a server it refuses.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cox_protocol::{Concurrency, Diff, Risk, Tool, ToolCx, ToolError, ToolOutput, ToolSpec};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::Value;
use similar::TextDiff;

use super::diag::{Diagnostic, Position, Range, path_for};
use super::nav::position;
use super::{LspPool, baseline, introduced, load, root_for};
use crate::path::confine;
use crate::write::{Rewrite, rewrite_all};

const NAME: &str = "rename_symbol";

/// One text replacement, as LSP's `TextEdit` (an `AnnotatedTextEdit`
/// parses as one too; its annotation is ignored).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}

/// The text edits for one document, with the version the server made them
/// against when it said (`documentChanges`), else `None` (`changes`).
#[derive(Debug, Clone, PartialEq)]
pub struct DocEdit {
    pub uri: String,
    pub version: Option<i32>,
    pub edits: Vec<TextEdit>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawWorkspaceEdit {
    changes: Option<BTreeMap<String, Vec<TextEdit>>>,
    document_changes: Option<Vec<Value>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDocEdit {
    text_document: RawVersioned,
    edits: Vec<TextEdit>,
}

#[derive(Deserialize)]
struct RawVersioned {
    uri: String,
    version: Option<i32>,
}

fn denied(why: String) -> ToolError {
    ToolError::Denied { why }
}

/// The text edits of a rename reply, in the order they apply. A file
/// create, rename or delete is refused: this tool only edits text.
pub fn parse_edit(reply: Value) -> Result<Vec<DocEdit>, ToolError> {
    if reply.is_null() {
        return Ok(Vec::new());
    }
    let unreadable = |e: serde_json::Error| denied(format!("unreadable rename edit: {e}"));
    let raw: RawWorkspaceEdit = serde_json::from_value(reply).map_err(unreadable)?;
    // `documentChanges` wins when both are sent, as the spec says.
    if let Some(changes) = raw.document_changes {
        return changes
            .into_iter()
            .map(|change| {
                if let Some(kind) = change.get("kind").and_then(Value::as_str) {
                    return Err(denied(format!(
                        "the server's rename also does a file `{kind}`; {NAME} only edits text"
                    )));
                }
                let doc: RawDocEdit = serde_json::from_value(change).map_err(unreadable)?;
                Ok(DocEdit {
                    uri: doc.text_document.uri,
                    version: doc.text_document.version,
                    edits: doc.edits,
                })
            })
            .collect();
    }
    Ok(raw
        .changes
        .unwrap_or_default()
        .into_iter()
        .map(|(uri, edits)| DocEdit {
            uri,
            version: None,
            edits,
        })
        .collect())
}

/// The byte offset of `at` in `text`. The character counts UTF-16 units and
/// one past the line's end is clamped to it, as LSP says; a line past the
/// end or a position inside a surrogate pair is `None`.
fn offset(text: &str, at: Position) -> Option<usize> {
    let mut start = 0;
    for _ in 0..at.line {
        start += text[start..].find('\n')? + 1;
    }
    let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    let line = &text[start..end];
    let line = line.strip_suffix('\r').unwrap_or(line);
    let mut units = 0u32;
    for (i, c) in line.char_indices() {
        if units == at.character {
            return Some(start + i);
        }
        if units > at.character {
            return None;
        }
        units += u32::try_from(c.len_utf16()).unwrap_or(2);
    }
    // Past the last unit is clamped; overshooting it means a split pair.
    (units <= at.character).then_some(start + line.len())
}

/// `text` with `edits` applied; they may come in any order but must not
/// overlap, since the spec leaves overlapping edits undefined.
fn apply(text: &str, edits: &[TextEdit]) -> Result<String, String> {
    let mut spans = Vec::with_capacity(edits.len());
    for e in edits {
        let (Some(from), Some(to)) = (offset(text, e.range.start), offset(text, e.range.end))
        else {
            return Err(format!(
                "an edit at line {} is outside the file",
                e.range.start.line + 1
            ));
        };
        if from > to {
            return Err(format!(
                "an edit at line {} ends before it starts",
                e.range.start.line + 1
            ));
        }
        spans.push((from, to, e.new_text.as_str()));
    }
    // Stable, so insertions at one point keep the server's order.
    spans.sort_by_key(|&(from, to, _)| (from, to));
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (from, to, new) in spans {
        if from < at {
            return Err("two edits overlap".to_string());
        }
        out.push_str(&text[at..from]);
        out.push_str(new);
        at = to;
    }
    out.push_str(&text[at..]);
    Ok(out)
}

/// Each file's pre-image and new text, in path order. `versions` are the
/// versions cox synced each file at; an edit for any other version was
/// made against text the file no longer holds.
fn plan(
    docs: Vec<DocEdit>,
    versions: &HashMap<PathBuf, i32>,
    cx: &ToolCx,
) -> Result<Vec<Rewrite>, ToolError> {
    let mut files: BTreeMap<PathBuf, Rewrite> = BTreeMap::new();
    for doc in docs {
        let Some(file) = path_for(&doc.uri) else {
            return Err(denied(format!("the rename edits {}, not a file", doc.uri)));
        };
        let path = confine(&cx.writable_roots, &cx.cwd, &file.to_string_lossy()).map_err(|_| {
            denied(format!(
                "the rename also edits {}, outside the writable workspace roots",
                file.display()
            ))
        })?;
        if let Some(version) = doc.version
            && versions.get(&path) != Some(&version)
        {
            return Err(denied(format!(
                "the server's edit for {} is for version {version}, not the text cox sent it; \
                 call {NAME} again",
                file.display()
            )));
        }
        let rewrite = match files.entry(path) {
            std::collections::btree_map::Entry::Occupied(o) => o.into_mut(),
            std::collections::btree_map::Entry::Vacant(v) => {
                let before = load(v.key())?;
                let path = v.key().clone();
                v.insert(Rewrite {
                    path,
                    after: before.clone(),
                    before,
                })
            }
        };
        // `documentChanges` may name a file twice; each applies to the last.
        rewrite.after = apply(&rewrite.after, &doc.edits)
            .map_err(|why| denied(format!("{}: {why}", file.display())))?;
    }
    Ok(files
        .into_values()
        .filter(|r| r.after != r.before)
        .collect())
}

/// What `rename_symbol` is asked.
#[derive(Debug, Deserialize, JsonSchema)]
struct RenameInput {
    /// File holding the symbol, relative to the session `cwd` or absolute within a workspace root.
    path: String,
    /// 1-based line of the symbol.
    line: u32,
    /// 1-based column (in characters) of any character of the symbol.
    col: u32,
    /// The symbol's new name.
    new_name: String,
}

/// `rename_symbol`, over the pool `diagnostics` starts servers in.
pub struct RenameTool {
    pool: Arc<LspPool>,
    /// How long to wait for each file's diagnostics after the change
    /// (T59.3's `lsp.after_edit_ms`); `None` when `lsp.after_edit` is off.
    wait: Option<Duration>,
    /// The terminal-text guard (`cox_sanitize::sanitize`), handed in because
    /// `cox-tools` may not depend on it: a server's text is untrusted.
    sanitize: fn(&str) -> String,
}

impl RenameTool {
    pub fn new(pool: Arc<LspPool>, wait: Option<Duration>, sanitize: fn(&str) -> String) -> Self {
        Self {
            pool,
            wait,
            sanitize,
        }
    }

    fn clean(&self, e: ToolError) -> ToolError {
        match e {
            ToolError::Denied { why } => denied((self.sanitize)(&why)),
            other => other,
        }
    }
}

#[async_trait]
impl Tool for RenameTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: NAME.to_string(),
            description: "Renames the symbol at a position everywhere the file's language \
                 server finds it, in one change across files. `line` and `col` are 1-based \
                 and point into the file as `read` shows it. Needs the file's language server \
                 to be running already (call `diagnostics` on a file of that language to \
                 start it); without one it refuses rather than rename by text search. Every \
                 file is written or none is, each one's old text archived. Refuses when the \
                 rename would create, move or delete a file, touch a file outside the \
                 writable workspace, or apply to text that changed since the server saw it."
                .to_string(),
            input_schema: serde_json::to_value(schema_for!(RenameInput)).unwrap_or(Value::Null),
            deferred: true,
            risk: Risk::Write,
            // The files it writes are known only once the server answers.
            concurrency: Concurrency::Exclusive,
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
        let input: RenameInput =
            serde_json::from_value(input).map_err(|e| denied(format!("invalid input: {e}")))?;
        let new_name = input.new_name.trim();
        if new_name.is_empty() {
            return Err(denied("new_name is empty".to_string()));
        }
        let path = confine(&cx.roots, &cx.cwd, &input.path)?;
        let text = load(&path)?;
        let at = position(&text, input.line, input.col)?;
        let Some(server) = self.pool.running_for(&path) else {
            return Err(denied(format!(
                "no language server runs for {}; call `diagnostics` on it to start one. \
                 {NAME} never renames by text search",
                input.path
            )));
        };
        if !server.can_rename() {
            return Err(denied(
                "the file's language server cannot rename".to_string(),
            ));
        }
        let root = root_for(&path, &cx.roots, &cx.cwd);
        let mut baselines: HashMap<PathBuf, Vec<Diagnostic>> = HashMap::new();
        // The rename syncs the file, so its baseline is taken first.
        self.baselines(&server, std::slice::from_ref(&path), &mut baselines, cx)
            .await;

        let answer = tokio::select! {
            r = server.rename(&path, &text, at, new_name) => r,
            () = cx.cancel.cancelled() => return Err(ToolError::Cancelled),
        };
        let (edit, versions) = match answer {
            Ok(Some(found)) => found,
            Ok(None) => {
                return Err(denied(format!(
                    "nothing to rename at {}:{}:{}",
                    input.path, input.line, input.col
                )));
            }
            Err(e) => return Err(self.clean(denied(format!("the server cannot rename: {e}")))),
        };
        let docs = parse_edit(edit).map_err(|e| self.clean(e))?;
        let rewrites = plan(docs, &versions, cx).map_err(|e| self.clean(e))?;
        if rewrites.is_empty() {
            return Ok(reply(
                format!("no change: the server renamed nothing to `{new_name}`"),
                None,
            ));
        }
        let paths: Vec<PathBuf> = rewrites.iter().map(|r| r.path.clone()).collect();
        self.baselines(&server, &paths, &mut baselines, cx).await;

        rewrite_all(NAME, &rewrites, cx).await?;

        let shown = |p: &Path| p.strip_prefix(&root).unwrap_or(p).display().to_string();
        let mut text = format!("renamed to `{new_name}` in {} file(s):", rewrites.len());
        let mut unified = String::new();
        for r in &rewrites {
            let label = shown(&r.path);
            text.push('\n');
            text.push_str(&label);
            unified.push_str(
                &TextDiff::from_lines(&r.before, &r.after)
                    .unified_diff()
                    .context_radius(3)
                    .header(&label, &label)
                    .to_string(),
            );
        }
        if let Some(wait) = self.wait {
            let added = async {
                let mut added = String::new();
                for r in &rewrites {
                    let Some(before) = baselines.get(&r.path) else {
                        continue;
                    };
                    let new = introduced(&r.path, &server, before, &root, wait, self.sanitize);
                    if let Some(new) = new.await {
                        added.push_str(&new);
                    }
                }
                added
            };
            tokio::select! {
                a = tokio::time::timeout(wait * 2, added) => text.push_str(&a.unwrap_or_default()),
                () = cx.cancel.cancelled() => {}
            }
        }
        let diff = Diff {
            path: PathBuf::from(shown(&rewrites[0].path)),
            unified,
        };
        Ok(reply((self.sanitize)(&text), Some(diff)))
    }
}

impl RenameTool {
    /// Records the diagnostics each of `paths` has before the change, for
    /// T59.3's delta, all within one deadline; a file without a baseline in
    /// time gets no delta rather than a guessed one.
    async fn baselines(
        &self,
        server: &super::server::Server,
        paths: &[PathBuf],
        into: &mut HashMap<PathBuf, Vec<Diagnostic>>,
        cx: &ToolCx,
    ) {
        let Some(wait) = self.wait else { return };
        let fill = async {
            for path in paths {
                if into.contains_key(path) {
                    continue;
                }
                if let Some(known) = baseline(server, path, wait).await {
                    into.insert(path.clone(), known);
                }
            }
        };
        tokio::select! {
            _ = tokio::time::timeout(wait * 2, fill) => {}
            () = cx.cancel.cancelled() => {}
        }
    }
}

fn reply(text: String, diff: Option<Diff>) -> ToolOutput {
    ToolOutput {
        text,
        is_error: false,
        diff,
        structured: None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use cox_protocol::{Archive, ArchiveId, ArchivePut, StoreError};
    use serde_json::json;

    use super::super::tests::{cx, tool, workspace};
    use super::super::{DiagnosticsTool, lock};
    use super::*;

    /// Stands in for `cox_sanitize::sanitize`, which `cox-tools` may not
    /// depend on; the session hands in the real one.
    fn strip_escapes(s: &str) -> String {
        s.replace('\u{1b}', "")
    }

    /// Keeps every archived pre-image as (subject, text).
    #[derive(Default)]
    struct Kept(Mutex<Vec<(String, String)>>);

    #[async_trait]
    impl Archive for Kept {
        async fn put(&self, put: ArchivePut) -> Result<ArchiveId, StoreError> {
            let text = String::from_utf8(put.bytes).unwrap();
            lock(&self.0).push((put.subject.unwrap_or_default(), text));
            Ok(ArchiveId::new())
        }
        async fn get(&self, _id: &ArchiveId) -> Result<Vec<u8>, StoreError> {
            Ok(Vec::new())
        }
    }

    const B: &str = "fn other() { main(); }\n";

    /// `src/a.rs` holding `a`, `src/b.rs` holding [`B`], and the server for
    /// them already started by `diagnostics`.
    async fn running(a: &str) -> (tempfile::TempDir, PathBuf, DiagnosticsTool) {
        let (dir, root) = workspace(&["src/a.rs", "src/b.rs"]);
        std::fs::write(root.join("src/a.rs"), a).unwrap();
        std::fs::write(root.join("src/b.rs"), B).unwrap();
        let (diags, _) = tool(&[("rust", "/bin/sh", "rs")]);
        let out = diags
            .call(json!({"path": "src/a.rs"}), &cx(&root))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.text);
        (dir, root, diags)
    }

    fn rename(diags: &DiagnosticsTool, wait: Option<Duration>) -> RenameTool {
        RenameTool::new(diags.pool.clone(), wait, strip_escapes)
    }

    fn to(new_name: &str) -> Value {
        json!({"path": "src/a.rs", "line": 1, "col": 4, "new_name": new_name})
    }

    fn read(root: &Path, file: &str) -> String {
        std::fs::read_to_string(root.join(file)).unwrap()
    }

    fn with_archive(root: &Path) -> (ToolCx, Arc<Kept>) {
        let kept = Arc::new(Kept::default());
        let mut cx = cx(root);
        cx.archive = kept.clone();
        (cx, kept)
    }

    #[tokio::test(start_paused = true)]
    async fn one_call_renames_across_two_files_and_archives_both() {
        let (_dir, root, diags) = running("fn main() {}\n").await;
        let (cx, kept) = with_archive(&root);

        let out = rename(&diags, None).call(to("start"), &cx).await.unwrap();
        assert_eq!(
            out.text,
            "renamed to `start` in 2 file(s):\nsrc/a.rs\nsrc/b.rs"
        );
        assert_eq!(read(&root, "src/a.rs"), "fn start() {}\n");
        assert_eq!(read(&root, "src/b.rs"), "fn other() { start(); }\n");
        let kept = lock(&kept.0).clone();
        assert_eq!(kept.len(), 2);
        assert!(kept[0].0.ends_with("src/a.rs") && kept[0].1 == "fn main() {}\n");
        assert!(kept[1].0.ends_with("src/b.rs") && kept[1].1 == B);
        let diff = out.diff.unwrap();
        assert!(
            diff.unified.contains("+fn other() { start(); }"),
            "{}",
            diff.unified
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_result_ends_with_the_diagnostics_the_rename_introduced() {
        let (_dir, root, diags) = running("fn main() {}\n").await;

        let wait = Some(Duration::from_secs(5));
        let out = rename(&diags, wait)
            .call(to("BAD"), &cx(&root))
            .await
            .unwrap();
        assert!(out.text.contains("new diagnostics ("), "{}", out.text);
        assert!(
            out.text.contains("src/a.rs:1:1: error: [31mbad token"),
            "{}",
            out.text
        );
        assert!(!out.text.contains('\u{1b}'), "sanitized: {}", out.text);
    }

    #[tokio::test(start_paused = true)]
    async fn without_a_running_server_it_refuses_and_writes_nothing() {
        let (_dir, root) = workspace(&["src/a.rs", "src/b.rs"]);
        let (diags, started) = tool(&[("rust", "/bin/sh", "rs")]);

        let out = rename(&diags, None).call(to("start"), &cx(&root)).await;
        assert!(
            matches!(&out, Err(ToolError::Denied { why }) if why.contains("no language server runs")),
            "{out:?}"
        );
        assert_eq!(read(&root, "src/a.rs"), "fn main() {}\n");
        assert!(lock(&started).is_empty(), "a rename never starts a server");
    }

    /// Each server answer that must leave every file as it was.
    #[tokio::test(start_paused = true)]
    async fn a_stale_create_outside_or_empty_answer_is_refused_whole() {
        for (marker, why) in [
            ("STALE", "is for version"),
            ("CREATE", "file `create`"),
            ("OUTSIDE", "outside the writable workspace roots"),
            ("NOWHERE", "nothing to rename at src/a.rs:1:4"),
        ] {
            let a = format!("fn main() {{}} // {marker}\n");
            let (_dir, root, diags) = running(&a).await;
            let (cx, kept) = with_archive(&root);

            let out = rename(&diags, None).call(to("start"), &cx).await;
            assert!(
                matches!(&out, Err(ToolError::Denied { why: w }) if w.contains(why)),
                "{marker}: {out:?}"
            );
            assert_eq!(read(&root, "src/a.rs"), a, "{marker}");
            assert_eq!(read(&root, "src/b.rs"), B, "{marker}");
            assert!(lock(&kept.0).is_empty(), "{marker}: nothing archived");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_file_changed_on_disk_is_resynced_before_the_rename() {
        let (_dir, root, diags) = running("fn main() {}\n").await;
        // The server has `b.rs` open as `B`; the disk now says otherwise.
        let b = cx(&root);
        diags.call(json!({"path": "src/b.rs"}), &b).await.unwrap();
        std::fs::write(root.join("src/b.rs"), "// moved\nfn x() { main(); }\n").unwrap();

        rename(&diags, None)
            .call(to("start"), &cx(&root))
            .await
            .unwrap();
        assert_eq!(read(&root, "src/b.rs"), "// moved\nfn x() { start(); }\n");
    }

    #[test]
    fn spec_is_a_deferred_exclusive_write_on_the_symbols_file() {
        let (diags, _) = tool(&[("rust", "/bin/sh", "rs")]);
        let tool = rename(&diags, None);
        let spec = tool.spec();
        assert_eq!(spec.name, "rename_symbol");
        assert!(spec.deferred);
        assert_eq!(spec.risk, Risk::Write);
        assert_eq!(spec.concurrency, Concurrency::Exclusive);
        assert_eq!(tool.subject(&to("x")), "src/a.rs");
        assert_eq!(
            tool.touches(&to("x")),
            None,
            "known only once the server answers"
        );
    }

    fn edit(sl: u32, sc: u32, el: u32, ec: u32, new: &str) -> TextEdit {
        let p = |line, character| Position { line, character };
        TextEdit {
            range: Range {
                start: p(sl, sc),
                end: p(el, ec),
            },
            new_text: new.to_string(),
        }
    }

    #[test]
    fn edits_apply_in_utf16_units_in_any_order() {
        // `𝒳` is two UTF-16 units, so `y` starts at unit 2.
        let text = "𝒳y z\r\nw\n";
        let out = apply(text, &[edit(1, 0, 1, 1, "W"), edit(0, 2, 0, 3, "Y")]).unwrap();
        assert_eq!(out, "𝒳Y z\r\nW\n");
        // Past the line's end is clamped to it, before the `\r`.
        assert_eq!(
            apply(text, &[edit(0, 9, 0, 9, "!")]).unwrap(),
            "𝒳y z!\r\nw\n"
        );
        assert_eq!(
            apply(text, &[edit(2, 0, 2, 0, "end")]).unwrap(),
            "𝒳y z\r\nw\nend"
        );
    }

    #[test]
    fn bad_edits_are_refused() {
        let text = "𝒳y\n";
        let refused = [
            vec![edit(0, 1, 0, 2, "")],
            vec![edit(5, 0, 5, 0, "")],
            vec![edit(0, 2, 0, 0, "")],
            vec![edit(0, 0, 0, 3, "a"), edit(0, 2, 0, 3, "b")],
        ];
        for edits in refused {
            assert!(apply(text, &edits).is_err(), "{edits:?}");
        }
    }

    #[test]
    fn plain_changes_parse_without_versions() {
        let p = json!({"line": 0, "character": 0});
        let one = json!([{"range": {"start": p, "end": p}, "newText": "x", "annotationId": "a"}]);
        let docs = parse_edit(json!({"changes": {"file:///a.rs": one}})).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].version, None);
        assert_eq!(docs[0].edits[0].new_text, "x");
        assert!(parse_edit(Value::Null).unwrap().is_empty());
        assert!(matches!(
            parse_edit(json!(7)),
            Err(ToolError::Denied { .. })
        ));
    }
}
