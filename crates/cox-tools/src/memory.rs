// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `memory_save` / `memory_search` (T10.1): durable project facts. Saving
//! writes `<name>.md` plus the `MEMORY.md` index line and upserts the
//! store's FTS rows (for T10.2's dedup reader); searching reads the FTS
//! hits first, then fills the rest from the files, top 5 with capped
//! excerpts. The file format mirrors `cox_ext::memory` (which this crate
//! may not depend on — plan.md dependency direction), kept in sync by the
//! roundtrip test below.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use cox_protocol::{
    Concurrency, MemoryHit, Risk, Store, Tool, ToolCx, ToolError, ToolOutput, ToolSpec,
};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::Value;

/// Cap for one search excerpt.
const EXCERPT_CHARS: usize = 500;

/// Default and maximum hits per search.
const DEFAULT_LIMIT: usize = 5;

/// `memory_save`: persist one fact under `dir`.
pub struct MemorySaveTool {
    store: Arc<dyn Store>,
    dir: PathBuf,
}

/// `memory_search`: find facts by query, FTS first then files.
pub struct MemorySearchTool {
    store: Arc<dyn Store>,
    dir: PathBuf,
}

impl MemorySaveTool {
    /// Serves saves into `dir`, indexing into `store`.
    pub fn new(store: Arc<dyn Store>, dir: PathBuf) -> Self {
        Self { store, dir }
    }
}

impl MemorySearchTool {
    /// Serves searches over `dir`, ranking `store` FTS hits first.
    pub fn new(store: Arc<dyn Store>, dir: PathBuf) -> Self {
        Self { store, dir }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SaveInput {
    /// Fact slug (`[a-z0-9-]`), the file stem.
    name: String,
    /// The fact body in full.
    body: String,
    /// One-line description; defaults to the body's first line.
    #[serde(default)]
    description: Option<String>,
    /// Fact kind; defaults to `fact`.
    #[serde(default, rename = "type")]
    kind: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SearchInput {
    /// Space-separated terms; every term must occur.
    query: String,
    /// Max hits (default 5, at most 10).
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl Tool for MemorySaveTool {
    fn spec(&self) -> ToolSpec {
        let input_schema = serde_json::to_value(schema_for!(SaveInput)).unwrap_or(Value::Null);
        ToolSpec {
            name: "memory_save".to_string(),
            description: "Saves one durable project fact (a decision, convention or gotcha \
                worth keeping across sessions) under its slug name with a one-line \
                description. Later turns find it through `memory_search`."
                .to_string(),
            input_schema,
            deferred: true,
            risk: Risk::Write,
            concurrency: Concurrency::Exclusive,
        }
    }

    fn subject(&self, input: &Value) -> String {
        input
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    async fn call(&self, input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let input: SaveInput = serde_json::from_value(input).map_err(|e| ToolError::Denied {
            why: format!("invalid memory_save input: {e}"),
        })?;
        if !is_valid_name(&input.name) {
            return Err(ToolError::Denied {
                why: format!("invalid memory name {:?}: use [a-z0-9-]", input.name),
            });
        }
        if input.body.trim().is_empty() {
            return Err(ToolError::Denied {
                why: "memory body is empty".to_string(),
            });
        }
        let description = input
            .description
            .map(|d| one_line(&d))
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| one_line(&input.body).chars().take(160).collect());
        let kind = input
            .kind
            .map(|k| one_line(&k))
            .filter(|k| !k.is_empty())
            .unwrap_or_else(|| "fact".to_string());
        fs::create_dir_all(&self.dir).map_err(|_| ToolError::Io)?;
        let rel = format!("{}.md", input.name);
        let text = format!(
            "---\nname: {}\ndescription: {description}\ntype: {kind}\n---\n{}",
            input.name,
            input.body.trim()
        );
        fs::write(self.dir.join(&rel), &text).map_err(|_| ToolError::Io)?;
        upsert_index(&self.dir, &input.name, &description).map_err(|_| ToolError::Io)?;
        self.store
            .memory_upsert(
                &slug_from_dir(&self.dir),
                &input.name,
                &rel,
                &kind,
                input.body.trim(),
            )
            .map_err(|_| ToolError::Io)?;
        Ok(ToolOutput {
            text: format!("saved memory `{}`", input.name),
            is_error: false,
            diff: None,
            structured: Some(serde_json::json!({"name": input.name, "path": rel})),
        })
    }
}

#[async_trait]
impl Tool for MemorySearchTool {
    fn spec(&self) -> ToolSpec {
        let input_schema = serde_json::to_value(schema_for!(SearchInput)).unwrap_or(Value::Null);
        ToolSpec {
            name: "memory_search".to_string(),
            description: "Searches saved project facts by query terms and returns up to five \
                hits with short excerpts. Use it when past decisions, conventions or gotchas \
                might already answer the current question."
                .to_string(),
            input_schema,
            deferred: true,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }

    fn subject(&self, input: &Value) -> String {
        input
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    async fn call(&self, input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let input: SearchInput = serde_json::from_value(input).map_err(|e| ToolError::Denied {
            why: format!("invalid memory_search input: {e}"),
        })?;
        if input.query.trim().is_empty() {
            return Err(ToolError::Denied {
                why: "memory_search query is empty".to_string(),
            });
        }
        let limit = input.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, 10);
        let mut hits: Vec<Hit> = self
            .store
            .memory_search(&input.query, limit)
            .unwrap_or_default()
            .into_iter()
            .map(|h: MemoryHit| Hit {
                name: h.name,
                excerpt: h.snippet.chars().take(EXCERPT_CHARS).collect(),
            })
            .collect();
        if hits.len() < limit {
            let seen: Vec<String> = hits.iter().map(|h| h.name.clone()).collect();
            hits.extend(scan_files(
                &self.dir,
                &input.query,
                limit - hits.len(),
                &seen,
            ));
        }
        if hits.is_empty() {
            return Ok(ToolOutput {
                text: format!("no memories match {:?}", input.query),
                is_error: false,
                diff: None,
                structured: Some(serde_json::json!({"hits": []})),
            });
        }
        let text = hits
            .iter()
            .map(|h| format!("## {}\n{}", h.name, h.excerpt))
            .collect::<Vec<_>>()
            .join("\n\n");
        Ok(ToolOutput {
            text,
            is_error: false,
            diff: None,
            structured: Some(serde_json::json!({
                "hits": hits.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(),
            })),
        })
    }
}

struct Hit {
    name: String,
    excerpt: String,
}

/// Fact slugs double as file stems (mirrors `cox_ext::memory::is_valid_name`;
/// crate direction forbids sharing the function).
fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The FTS project key for a memory dir: `<slug>` from `…/<slug>/memory`.
fn slug_from_dir(dir: &Path) -> String {
    dir.parent()
        .filter(|_| dir.file_name().is_some_and(|n| n == "memory"))
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "default".to_string())
}

/// Adds or replaces the fact's `MEMORY.md` line.
fn upsert_index(dir: &Path, name: &str, description: &str) -> Result<(), String> {
    let path = dir.join("MEMORY.md");
    let current = fs::read_to_string(&path).unwrap_or_else(|_| "# Memory\n".to_string());
    let prefix = format!("- [{name}](");
    let mut lines: Vec<String> = current
        .lines()
        .filter(|l| !l.starts_with(&prefix))
        .map(str::to_string)
        .collect();
    if lines.first().is_none_or(|l| l != "# Memory") {
        lines.insert(0, "# Memory".to_string());
    }
    lines.push(format!("- [{name}]({name}.md) — {description}"));
    fs::write(path, lines.join("\n") + "\n").map_err(|e| e.to_string())
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// File-backed search over one memory dir, skipping `seen` names.
fn scan_files(dir: &Path, query: &str, limit: usize, seen: &[String]) -> Vec<Hit> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if terms.is_empty() || limit == 0 {
        return Vec::new();
    }
    let Ok(files) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = files
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "md")
                && p.file_name().is_some_and(|n| n != "MEMORY.md")
                && p.is_file()
        })
        .collect();
    paths.sort();
    let mut scored: Vec<(usize, Hit)> = Vec::new();
    for path in paths {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let (name, body) = split_fact(&text);
        let name = name.unwrap_or_else(|| {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        if seen.iter().any(|s| s == &name) {
            continue;
        }
        let hay = format!("{name}\n{body}").to_lowercase();
        if !terms.iter().all(|t| hay.contains(t)) {
            continue;
        }
        let score: usize = terms.iter().map(|t| hay.matches(t).count()).sum();
        scored.push((
            score,
            Hit {
                name,
                excerpt: excerpt(&body, &terms),
            },
        ));
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.name.cmp(&b.1.name)));
    scored.into_iter().take(limit).map(|(_, hit)| hit).collect()
}

/// Splits `<frontmatter>---<body>`; reads `name:` without a YAML dependency
/// (the writer above controls the format, so prefix matching is enough).
fn split_fact(text: &str) -> (Option<String>, String) {
    let mut lines = text.lines();
    if lines.next() != Some("---") {
        return (None, text.trim().to_string());
    }
    let mut name = None;
    let mut body = Vec::new();
    let mut in_header = true;
    for line in lines {
        if in_header && line == "---" {
            in_header = false;
            continue;
        }
        if in_header {
            if let Some(value) = line.strip_prefix("name:") {
                name = Some(value.trim().to_string());
            }
        } else {
            body.push(line);
        }
    }
    if in_header {
        return (None, text.trim().to_string());
    }
    (
        name.filter(|n| is_valid_name(n)),
        body.join("\n").trim().to_string(),
    )
}

/// Body window around the first term hit, capped.
fn excerpt(body: &str, terms: &[String]) -> String {
    let lower = body.to_lowercase();
    let at = terms
        .iter()
        .filter_map(|t| lower.find(t))
        .min()
        .unwrap_or(0);
    let start = at.saturating_sub(200);
    let window: String = body.chars().skip(start).take(EXCERPT_CHARS).collect();
    if start > 0 {
        format!("…{window}")
    } else {
        window
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use cox_protocol::{Archive, ArchiveId, ArchivePut, MemoryHit, SessionId, StoreError};

    /// In-memory `Store` double: `memory_*` over a map, everything else inert.
    struct FakeStore {
        memory: Mutex<HashMap<(String, String), (String, String)>>,
    }

    impl FakeStore {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                memory: Mutex::new(HashMap::new()),
            })
        }

        fn seed(self: &Arc<Self>, project: &str, name: &str, body: &str) {
            self.memory.lock().unwrap().insert(
                (project.into(), name.into()),
                (format!("{name}.md"), body.into()),
            );
        }
    }

    impl Store for FakeStore {
        fn open(_home: &Path) -> Result<Self, StoreError>
        where
            Self: Sized,
        {
            Ok(Self {
                memory: Mutex::new(HashMap::new()),
            })
        }
        fn session_create(&self, _s: &cox_protocol::SessionRow) -> Result<(), StoreError> {
            Ok(())
        }
        fn rollout_append(
            &self,
            _id: &SessionId,
            _ev: &cox_protocol::Event,
        ) -> Result<u64, StoreError> {
            Ok(0)
        }
        fn rollout_read(&self, _id: &SessionId) -> Result<Vec<cox_protocol::Event>, StoreError> {
            Ok(vec![])
        }
        fn usage_insert(&self, _row: &cox_protocol::UsageRow) -> Result<(), StoreError> {
            Ok(())
        }
        fn archive_put(&self, _a: &ArchivePut) -> Result<ArchiveId, StoreError> {
            Ok(ArchiveId::new())
        }
        fn archive_get(&self, _id: &ArchiveId) -> Result<Vec<u8>, StoreError> {
            Ok(vec![])
        }
        fn memory_search(&self, q: &str, limit: usize) -> Result<Vec<MemoryHit>, StoreError> {
            let terms: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
            let mut hits = vec![];
            for ((_, name), (path, body)) in self.memory.lock().unwrap().iter() {
                if terms.iter().all(|t| body.to_lowercase().contains(t)) {
                    hits.push(MemoryHit {
                        name: name.clone(),
                        path: path.into(),
                        snippet: body.chars().take(200).collect(),
                    });
                }
                if hits.len() >= limit.max(1) {
                    break;
                }
            }
            Ok(hits)
        }
        fn memory_upsert(
            &self,
            project: &str,
            name: &str,
            path: &str,
            _kind: &str,
            body: &str,
        ) -> Result<(), StoreError> {
            self.memory
                .lock()
                .unwrap()
                .insert((project.into(), name.into()), (path.into(), body.into()));
            Ok(())
        }
        fn rollout_index(
            &self,
            _session: &cox_protocol::SessionId,
            _turn: u32,
            _text: &str,
        ) -> Result<(), StoreError> {
            Ok(())
        }
        fn checkpoint_insert(&self, _row: &cox_protocol::CheckpointRow) -> Result<(), StoreError> {
            Ok(())
        }
        fn checkpoint_list(
            &self,
            _session: &SessionId,
        ) -> Result<Vec<cox_protocol::CheckpointRow>, StoreError> {
            Ok(vec![])
        }
    }

    struct NoopArchive;

    #[async_trait]
    impl Archive for NoopArchive {
        async fn put(&self, _put: ArchivePut) -> Result<ArchiveId, StoreError> {
            Ok(ArchiveId::new())
        }
        async fn get(&self, _id: &ArchiveId) -> Result<Vec<u8>, StoreError> {
            Ok(Vec::new())
        }
    }

    fn cx() -> ToolCx {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        crate::tool_cx(
            vec![PathBuf::from("/tmp")],
            PathBuf::from("/tmp"),
            cox_protocol::SandboxPolicy {
                mode: cox_protocol::SandboxMode::ReadOnly,
                network: false,
                writable: vec![],
                readonly_in_workspace: vec![],
                linux_backend: Default::default(),
            },
            Arc::new(NoopArchive),
            tokio_util::sync::CancellationToken::new(),
            tx,
            SessionId::new(),
            cox_protocol::CallId::new(),
        )
    }

    #[tokio::test]
    async fn memory_save_writes_file_index_and_store() {
        let dir = tempfile::tempdir().unwrap();
        let mem = dir.path().join("proj").join("memory");
        let store = FakeStore::new();
        let tool = MemorySaveTool::new(store.clone(), mem.clone());
        let out = tool
            .call(
                serde_json::json!({
                    "name": "auth-flow",
                    "body": "Login goes through auth.rs.",
                    "description": "Auth entry point.",
                    "type": "decision",
                }),
                &cx(),
            )
            .await
            .expect("save");
        assert!(out.text.contains("auth-flow"), "{}", out.text);
        let file = fs::read_to_string(mem.join("auth-flow.md")).expect("fact file");
        assert!(file.contains("name: auth-flow"), "{file}");
        let index = fs::read_to_string(mem.join("MEMORY.md")).expect("index");
        assert!(index.contains("- [auth-flow](auth-flow.md)"), "{index}");
        assert!(
            store
                .memory
                .lock()
                .unwrap()
                .contains_key(&("proj".into(), "auth-flow".into()))
        );
    }

    #[tokio::test]
    async fn memory_search_finds_saved_fact() {
        let dir = tempfile::tempdir().unwrap();
        let mem = dir.path().join("memory");
        let store = FakeStore::new();
        let save = MemorySaveTool::new(store.clone(), mem.clone());
        for (name, body) in [
            ("auth-flow", "Login goes through auth.rs with sessions."),
            ("widget-api", "Canvas holds widgets by id."),
        ] {
            save.call(serde_json::json!({"name": name, "body": body}), &cx())
                .await
                .expect("save");
        }
        // The store holds both, but only the file scan path is asserted here:
        // drain the store so the files must answer.
        store.memory.lock().unwrap().clear();
        let search = MemorySearchTool::new(store, mem);
        let out = search
            .call(serde_json::json!({"query": "sessions auth"}), &cx())
            .await
            .expect("search");
        assert!(out.text.contains("auth-flow"), "{}", out.text);
        assert!(!out.text.contains("widget-api"), "{}", out.text);
    }

    #[tokio::test]
    async fn memory_search_prefers_store_hits_then_files() {
        let dir = tempfile::tempdir().unwrap();
        let mem = dir.path().join("memory");
        let store = FakeStore::new();
        store.seed("proj", "indexed-fact", "the query term appears here");
        fs::create_dir_all(&mem).unwrap();
        fs::write(
            mem.join("file-fact.md"),
            "---\nname: file-fact\ndescription: d\ntype: fact\n---\nquery term in a file",
        )
        .unwrap();
        let search = MemorySearchTool::new(store, mem);
        let out = search
            .call(serde_json::json!({"query": "query term"}), &cx())
            .await
            .expect("search");
        let first = out.text.find("indexed-fact").unwrap_or(usize::MAX);
        let second = out.text.find("file-fact").unwrap_or(usize::MAX);
        assert!(first < second, "store hit first:\n{}", out.text);
    }

    #[tokio::test]
    async fn memory_rejects_bad_names() {
        let dir = tempfile::tempdir().unwrap();
        let store = FakeStore::new();
        let tool = MemorySaveTool::new(store, dir.path().to_path_buf());
        for bad in ["../x", "UPPER", "has space", ""] {
            assert!(
                tool.call(serde_json::json!({"name": bad, "body": "b"}), &cx())
                    .await
                    .is_err(),
                "{bad}"
            );
        }
    }
}
