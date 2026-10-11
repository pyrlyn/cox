// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Deferred crate documentation (T65.1, D6d). `docs_resolve` reads
//! `Cargo.lock` from the workspace roots, `docs_query` searches a local
//! `items.jsonl` cache (or a root `llms.txt`), and only `docs_fetch` opens
//! a socket — one GET of a docs.rs rustdoc, with no API key. Separate from
//! `memory` so project facts and crate docs do not share a store, and
//! separate from `web_fetch` so a query cannot be turned into a download.

#[cfg(test)]
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use cox_protocol::{Concurrency, Risk, Tool, ToolCx, ToolError, ToolOutput, ToolSpec};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::write::atomic_write;

/// Hits one query returns. The core archives `ToolOutput.text`; this cap is
/// per hit, not a second cut of the whole result.
const MAX_HITS: usize = 5;
const EXCERPT_CHARS: usize = 400;
/// docs.rs rustdoc is larger than `web_fetch`'s page cap; still bounded so
/// one response cannot fill the disk.
const MAX_DOCS_BYTES: usize = 64 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const DOCS_HOST: &str = "https://docs.rs";

// Counts `DocsFetchTool` constructors on this thread. `docs_query` must
// not bump it: a cache miss is not a download.
#[cfg(test)]
thread_local! {
    static FETCH_CONSTRUCTED: Cell<u32> = const { Cell::new(0) };
}

fn note_fetch_constructed() {
    #[cfg(test)]
    FETCH_CONSTRUCTED.with(|n| n.set(n.get().saturating_add(1)));
}

/// `docs_resolve`: crate name and version from `Cargo.lock`.
pub struct DocsResolveTool;

/// `docs_query`: cached rustdoc snippets for one crate version.
pub struct DocsQueryTool {
    /// `~/.cox/docs`, or a test directory. A version directory under
    /// [`Self::rtok`] wins when it exists.
    cache: PathBuf,
    /// `~/.rtok/docs` when this tool follows the home rule; `None` in tests
    /// that pass one directory.
    rtok: Option<PathBuf>,
}

/// `docs_fetch`: one docs.rs download into the same cache `docs_query` reads.
pub struct DocsFetchTool {
    cache: PathBuf,
    rtok: Option<PathBuf>,
    /// `https://docs.rs` in production. Tests point it at a loopback server.
    base: String,
    client: cox_web::Client,
}

impl DocsQueryTool {
    /// Searches `cache` only.
    pub fn new(cache: PathBuf) -> Self {
        Self { cache, rtok: None }
    }

    /// `~/.rtok/docs/<name>/<version>` when that directory exists, otherwise
    /// `~/.cox/docs`.
    pub fn from_home() -> Self {
        let (cache, rtok) = home_caches();
        Self { cache, rtok }
    }

    fn items_file(&self, name: &str, version: &str) -> Option<PathBuf> {
        let path =
            version_dir(&self.cache, self.rtok.as_deref(), name, version).join("items.jsonl");
        path.is_file().then_some(path)
    }
}

impl DocsFetchTool {
    /// Writes into `cache` and downloads from docs.rs.
    pub fn new(cache: PathBuf) -> Self {
        Self::build(cache, None, DOCS_HOST.to_string(), cox_web::client())
    }

    /// Same cache rule as [`DocsQueryTool::from_home`].
    pub fn from_home() -> Self {
        let (cache, rtok) = home_caches();
        Self::build(cache, rtok, DOCS_HOST.to_string(), cox_web::client())
    }

    fn build(cache: PathBuf, rtok: Option<PathBuf>, base: String, client: cox_web::Client) -> Self {
        note_fetch_constructed();
        Self {
            cache,
            rtok,
            base,
            client,
        }
    }

    #[cfg(test)]
    fn for_tests(cache: PathBuf, base: String) -> Self {
        Self::build(cache, None, base, cox_web::client_for_tests())
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ResolveInput {
    /// Crate name as it appears in `Cargo.lock`.
    name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct QueryInput {
    /// Crate name, or `llms` to search a workspace `llms.txt`.
    name: String,
    /// Space-separated terms; every term must occur.
    query: String,
    /// Version from `docs_resolve`. Omit to use the lockfile.
    #[serde(default)]
    version: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct FetchInput {
    /// Crate name.
    name: String,
    /// Version from `docs_resolve`. Used only when the lockfile has no entry.
    #[serde(default)]
    version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Item {
    path: String,
    #[serde(default = "default_kind")]
    kind: String,
    #[serde(default)]
    docs: String,
}

fn default_kind() -> String {
    "item".to_string()
}

fn schema(value: schemars::Schema) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn output(text: String) -> ToolOutput {
    ToolOutput {
        text,
        is_error: false,
        diff: None,
        structured: None,
    }
}

fn denied(why: impl Into<String>) -> ToolError {
    ToolError::Denied { why: why.into() }
}

fn home_dir() -> PathBuf {
    // cox-tools cannot depend on cox-config; this is the same HOME /
    // USERPROFILE rule `cox_config::load::home_dir` uses.
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn home_caches() -> (PathBuf, Option<PathBuf>) {
    let home = home_dir();
    (
        home.join(".cox").join("docs"),
        Some(home.join(".rtok").join("docs")),
    )
}

/// A single path segment. Crate names and versions become directories under
/// the cache, so `..` and separators never leave it.
fn component(value: &str) -> Result<&str, ToolError> {
    let ok = !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains(['/', '\\', '\0'])
        && !value.contains("..");
    if ok {
        Ok(value)
    } else {
        Err(denied(format!("invalid crate name or version {value:?}")))
    }
}

fn version_dir(cache: &Path, rtok: Option<&Path>, name: &str, version: &str) -> PathBuf {
    if let Some(rtok) = rtok {
        let dir = rtok.join(name).join(version);
        if dir.is_dir() {
            return dir;
        }
    }
    cache.join(name).join(version)
}

fn quoted(value: &str) -> Option<String> {
    let rest = value.trim().strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Package rows in `Cargo.lock`. The unquoted `version = 4` header is not a
/// package version; nothing here runs `cargo`.
fn versions_in_lock(text: &str, name: &str) -> Vec<String> {
    let mut versions = Vec::new();
    let mut in_package = false;
    let mut pkg_name: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            in_package = true;
            pkg_name = None;
            continue;
        }
        if line.starts_with('[') {
            in_package = false;
            pkg_name = None;
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(rest) = line.strip_prefix("name = ") {
            pkg_name = quoted(rest);
        } else if let Some(rest) = line.strip_prefix("version = ")
            && pkg_name.as_deref() == Some(name)
            && let Some(version) = quoted(rest)
            && !versions.contains(&version)
        {
            versions.push(version);
        }
    }
    versions
}

fn lockfile_versions(roots: &[PathBuf], name: &str) -> Vec<String> {
    let mut found = Vec::new();
    for root in roots {
        let Ok(text) = fs::read_to_string(root.join("Cargo.lock")) else {
            continue;
        };
        for version in versions_in_lock(&text, name) {
            if !found.contains(&version) {
                found.push(version);
            }
        }
    }
    found
}

fn only_lock_version(locked: &[String]) -> Result<String, ToolError> {
    match locked {
        [one] => Ok(one.clone()),
        [] => Err(denied(
            "no lockfile entry; pass the version from docs_resolve, never latest",
        )),
        many => Err(denied(format!(
            "lockfile has {}; pass one version from docs_resolve",
            many.join(", ")
        ))),
    }
}

/// Lockfile version when the crate is locked. `latest` is never fetched.
/// A caller-supplied version is used only when the lockfile has no entry,
/// or when it is one of the locked versions.
fn version_to_fetch(
    roots: &[PathBuf],
    name: &str,
    passed: Option<&str>,
) -> Result<String, ToolError> {
    let locked = lockfile_versions(roots, name);
    let passed = passed.map(str::trim).filter(|v| !v.is_empty());
    let Some(version) = passed else {
        return only_lock_version(&locked);
    };
    if version == "latest" {
        return match locked.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err(denied(
                "refusing version \"latest\"; pass the version from docs_resolve",
            )),
            many => Err(denied(format!(
                "lockfile has {}; pass one version from docs_resolve",
                many.join(", ")
            ))),
        };
    }
    if locked.is_empty() || locked.iter().any(|v| v == version) {
        return Ok(version.to_string());
    }
    only_lock_version(&locked)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn docs_rs_url(name: &str, version: &str) -> String {
    if version.is_empty() {
        format!("{DOCS_HOST}/crate/{name}/json.zst")
    } else {
        format!("{DOCS_HOST}/crate/{name}/{version}/json.zst")
    }
}

/// Context7's `libraryName` / `question` inputs, renamed onto this tool's
/// `name` / `query` when the first parse fails. Scoped to `docs_query`.
fn parse_query(input: Value) -> Result<QueryInput, ToolError> {
    let invalid = |err: serde_json::Error| denied(format!("invalid docs_query input: {err}"));
    match serde_json::from_value(input.clone()) {
        Ok(parsed) => Ok(parsed),
        Err(first) => {
            let Some(mut obj) = input.as_object().cloned() else {
                return Err(invalid(first));
            };
            let lacks_name = !obj.contains_key("name");
            let lacks_query = !obj.contains_key("query");
            let aliased = obj.contains_key("libraryName") || obj.contains_key("question");
            if !(aliased && (lacks_name || lacks_query)) {
                return Err(invalid(first));
            }
            if lacks_name && let Some(value) = obj.remove("libraryName") {
                obj.insert("name".to_string(), value);
            }
            if lacks_query && let Some(value) = obj.remove("question") {
                obj.insert("query".to_string(), value);
            }
            serde_json::from_value(Value::Object(obj)).map_err(invalid)
        }
    }
}

fn load_items(path: &Path) -> Vec<Item> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Every query term must occur, case-folded, same rule as `memory_search`.
/// The score is the sum of term counts; ties break by path.
fn rank<'a>(items: &'a [Item], query: &str) -> Vec<&'a Item> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(usize, &Item)> = items
        .iter()
        .filter_map(|item| {
            let hay = format!("{} {} {}", item.path, item.kind, item.docs).to_lowercase();
            if !terms.iter().all(|term| hay.contains(term)) {
                return None;
            }
            let score = terms
                .iter()
                .map(|term| hay.matches(term.as_str()).count())
                .sum();
            Some((score, item))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.path.cmp(&b.1.path)));
    scored
        .into_iter()
        .take(MAX_HITS)
        .map(|(_, item)| item)
        .collect()
}

fn excerpt(docs: &str) -> String {
    docs.chars().take(EXCERPT_CHARS).collect()
}

fn format_hits(hits: &[&Item]) -> String {
    hits.iter()
        .map(|item| {
            let body = excerpt(&item.docs);
            if body.is_empty() {
                format!("{} {}", item.path, item.kind)
            } else {
                format!("{} {}\n{body}", item.path, item.kind)
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn finish(hits: Vec<&Item>, query: &str) -> ToolOutput {
    if hits.is_empty() {
        output(format!("no docs match {query:?}"))
    } else {
        output(format_hits(&hits))
    }
}

fn atx_heading(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = trimmed[hashes..].strip_prefix(' ')?;
    let title = rest.trim().trim_end_matches('#').trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

fn push_section(items: &mut Vec<Item>, path: &str, body: &str) {
    let docs = body.trim().to_string();
    if path.is_empty() {
        if !docs.is_empty() {
            items.push(Item {
                path: "llms".to_string(),
                kind: "section".to_string(),
                docs,
            });
        }
        return;
    }
    items.push(Item {
        path: path.to_string(),
        kind: "section".to_string(),
        docs,
    });
}

fn llms_sections(text: &str) -> Vec<Item> {
    let mut items = Vec::new();
    let mut path = String::new();
    let mut body = String::new();
    for line in text.lines() {
        if let Some(title) = atx_heading(line) {
            push_section(&mut items, &path, &body);
            path = title;
            body.clear();
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    push_section(&mut items, &path, &body);
    items
}

fn llms_in_roots(roots: &[PathBuf]) -> Vec<Item> {
    let mut items = Vec::new();
    for root in roots {
        let Ok(text) = fs::read_to_string(root.join("llms.txt")) else {
            continue;
        };
        items.extend(llms_sections(&text));
    }
    items
}

fn query_versions(
    roots: &[PathBuf],
    name: &str,
    passed: Option<&str>,
) -> Result<Vec<String>, ToolError> {
    if let Some(version) = passed.map(str::trim).filter(|v| !v.is_empty()) {
        return Ok(vec![component(version)?.to_string()]);
    }
    Ok(lockfile_versions(roots, name))
}

fn kind_str(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(kind)) => kind.clone(),
        Some(Value::Object(map)) => map.keys().next().cloned().unwrap_or_else(default_kind),
        _ => default_kind(),
    }
}

fn join_path(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("::")
        })
        .unwrap_or_default()
}

fn row(path: &str, kind: &str, docs: &str) -> Result<String, ToolError> {
    serde_json::to_string(&serde_json::json!({
        "path": path,
        "kind": kind,
        "docs": docs,
    }))
    .map_err(|_| ToolError::Io)
}

/// rustdoc JSON (`paths` + `index`) into the `items.jsonl` cache format.
fn items_jsonl(bytes: &[u8]) -> Result<String, ToolError> {
    let json: Value = serde_json::from_slice(bytes).map_err(|_| ToolError::Io)?;
    let mut lines = Vec::new();
    if let Some(paths) = json.get("paths").and_then(Value::as_object) {
        let mut ids: Vec<&str> = paths.keys().map(String::as_str).collect();
        ids.sort_unstable();
        for id in ids {
            let Some(entry) = paths.get(id) else {
                continue;
            };
            let path = join_path(entry.get("path"));
            if path.is_empty() {
                continue;
            }
            let kind = kind_str(entry.get("kind"));
            let docs = json
                .get("index")
                .and_then(|index| index.get(id))
                .and_then(|item| item.get("docs"))
                .and_then(Value::as_str)
                .unwrap_or("");
            lines.push(row(&path, &kind, docs)?);
        }
    }
    if lines.is_empty()
        && let Some(index) = json.get("index").and_then(Value::as_object)
    {
        let mut ids: Vec<&str> = index.keys().map(String::as_str).collect();
        ids.sort_unstable();
        for id in ids {
            let Some(item) = index.get(id) else {
                continue;
            };
            let path = item.get("name").and_then(Value::as_str).unwrap_or("");
            if path.is_empty() {
                continue;
            }
            let kind = kind_str(item.get("inner"));
            let docs = item.get("docs").and_then(Value::as_str).unwrap_or("");
            lines.push(row(path, &kind, docs)?);
        }
    }
    let mut out = lines.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    Ok(out)
}

fn net_err(timeout: bool) -> ToolError {
    if timeout {
        ToolError::Timeout
    } else {
        ToolError::Io
    }
}

/// One GET. A non-success status returns before any cache write, so a
/// previous file stays. No `Authorization` header is set.
async fn download(
    client: &cox_web::Client,
    url: &str,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, ToolError> {
    let pending = client.get(url).timeout(FETCH_TIMEOUT).send();
    let mut response = tokio::select! {
        _ = cancel.cancelled() => return Err(ToolError::Cancelled),
        result = pending => result.map_err(|err| net_err(err.is_timeout()))?,
    };
    let status = response.status();
    if !status.is_success() {
        return Err(denied(format!("{url} answered HTTP {status}")));
    }
    let mut body = Vec::new();
    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => return Err(ToolError::Cancelled),
            next = response.chunk() => next.map_err(|err| net_err(err.is_timeout()))?,
        };
        let Some(chunk) = chunk else {
            break;
        };
        if body.len().saturating_add(chunk.len()) > MAX_DOCS_BYTES {
            return Err(ToolError::TooLarge {
                bytes: body.len().saturating_add(chunk.len()) as u64,
                cap: MAX_DOCS_BYTES as u64,
            });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn write_cache(dir: &Path, bytes: &[u8], jsonl: &str) -> Result<(), ToolError> {
    atomic_write(&dir.join("json.zst"), bytes)?;
    atomic_write(
        dir.join("json.zst.sha256").as_path(),
        sha256_hex(bytes).as_bytes(),
    )?;
    atomic_write(&dir.join("items.jsonl"), jsonl.as_bytes())?;
    Ok(())
}

#[async_trait]
impl Tool for DocsResolveTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "docs_resolve".to_string(),
            description: "Exact crate name and version from the workspace Cargo.lock.".to_string(),
            input_schema: schema(schema_for!(ResolveInput)),
            deferred: true,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }

    fn subject(&self, input: &Value) -> String {
        input
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let input: ResolveInput = serde_json::from_value(input)
            .map_err(|err| denied(format!("invalid docs_resolve input: {err}")))?;
        let name = component(&input.name)?;
        let versions = lockfile_versions(&cx.roots, name);
        let text = if versions.is_empty() {
            "not in lockfile".to_string()
        } else {
            versions
                .iter()
                .map(|version| format!("cargo/{name}/{version}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok(output(text))
    }
}

#[async_trait]
impl Tool for DocsQueryTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "docs_query".to_string(),
            description: "Search cached rustdoc snippets for one crate version. \
                Use docs_resolve first. Returns at most 5 hits, 400 characters each."
                .to_string(),
            input_schema: schema(schema_for!(QueryInput)),
            deferred: true,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }

    fn subject(&self, input: &Value) -> String {
        input
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let input = parse_query(input)?;
        let query = input.query.trim();
        if query.is_empty() {
            return Err(denied("docs_query query is empty"));
        }
        let name = component(&input.name)?;
        if name == "llms" {
            let items = llms_in_roots(&cx.roots);
            if items.is_empty() {
                return Ok(output("no llms.txt in the workspace".to_string()));
            }
            return Ok(finish(rank(&items, query), query));
        }
        let versions = query_versions(&cx.roots, name, input.version.as_deref())?;
        if versions.is_empty() {
            return Ok(output("not cached".to_string()));
        }
        let mut items = Vec::new();
        let mut found = false;
        for version in &versions {
            let version = component(version)?;
            let Some(path) = self.items_file(name, version) else {
                continue;
            };
            found = true;
            items.extend(load_items(&path));
        }
        if !found {
            return Ok(output("not cached".to_string()));
        }
        Ok(finish(rank(&items, query), query))
    }
}

#[async_trait]
impl Tool for DocsFetchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "docs_fetch".to_string(),
            description: "Download rustdoc for one crate version into the local cache \
                from docs.rs. Pass the version from docs_resolve. One request, no API key."
                .to_string(),
            input_schema: schema(schema_for!(FetchInput)),
            deferred: true,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }

    fn subject(&self, input: &Value) -> String {
        let name = input.get("name").and_then(Value::as_str).unwrap_or("");
        let version = input.get("version").and_then(Value::as_str).unwrap_or("");
        docs_rs_url(name, version)
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let input: FetchInput = serde_json::from_value(input)
            .map_err(|err| denied(format!("invalid docs_fetch input: {err}")))?;
        let name = component(&input.name)?;
        let resolved = version_to_fetch(&cx.roots, name, input.version.as_deref())?;
        let version = component(&resolved)?;
        let url = format!(
            "{}/crate/{name}/{version}/json.zst",
            self.base.trim_end_matches('/')
        );
        // Decode before any rename, so a bad body leaves the previous files.
        let bytes = download(&self.client, &url, &cx.cancel).await?;
        let json = zstd::stream::decode_all(bytes.as_slice()).map_err(|_| ToolError::Io)?;
        let jsonl = items_jsonl(&json)?;
        let dir = version_dir(&self.cache, self.rtok.as_deref(), name, version);
        write_cache(&dir, &bytes, &jsonl)?;
        let count = jsonl.lines().filter(|line| !line.is_empty()).count();
        Ok(output(format!(
            "cached cargo/{name}/{version} ({count} items)"
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;
    use std::io::{Read, Write};

    use cox_protocol::{
        Archive, ArchiveId, ArchivePut, SandboxMode, SandboxPolicy, SessionId, StoreError,
    };
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    use super::*;

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

    fn context(root: &Path) -> ToolCx {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        crate::tool_cx(
            vec![root.to_path_buf()],
            root.to_path_buf(),
            SandboxPolicy {
                mode: SandboxMode::ReadOnly,
                network: false,
                writable: vec![],
                readonly_in_workspace: vec![],
                linux_backend: Default::default(),
            },
            std::sync::Arc::new(NoopArchive),
            CancellationToken::new(),
            tx,
            SessionId::new(),
            cox_protocol::CallId::new(),
        )
    }

    fn lockfile(root: &Path, body: &str) {
        fs::write(root.join("Cargo.lock"), body).expect("lockfile");
    }

    const LOCK: &str = "\
version = 4

[[package]]
name = \"serde\"
version = \"1.0.210\"

[[package]]
name = \"other\"
version = \"0.1.0\"
";

    fn write_rows(dir: &Path, rows: &str) {
        fs::create_dir_all(dir).expect("cache dir");
        fs::write(dir.join("items.jsonl"), rows).expect("jsonl");
    }

    #[test]
    fn docs_tools_are_deferred() {
        let specs = [
            DocsResolveTool.spec(),
            DocsQueryTool::new(PathBuf::new()).spec(),
            DocsFetchTool::new(PathBuf::new()).spec(),
        ];
        assert!(
            specs
                .iter()
                .all(|spec| spec.deferred && spec.risk == Risk::ReadOnly)
        );
        assert_eq!(
            specs
                .iter()
                .map(|spec| spec.name.as_str())
                .collect::<Vec<_>>(),
            ["docs_resolve", "docs_query", "docs_fetch"]
        );
    }

    #[tokio::test]
    async fn resolve_reads_the_lockfile_and_not_a_cargo_process() {
        let tmp = tempfile::tempdir().expect("tempdir");
        lockfile(tmp.path(), LOCK);
        let cx = context(tmp.path());
        let out = DocsResolveTool
            .call(json!({"name": "serde"}), &cx)
            .await
            .expect("resolve");
        assert_eq!(out.text, "cargo/serde/1.0.210");
        assert!(!out.is_error);
        let missing = DocsResolveTool
            .call(json!({"name": "nope"}), &cx)
            .await
            .expect("miss");
        assert_eq!(missing.text, "not in lockfile");
        let err = DocsResolveTool
            .call(json!({"name": "../serde"}), &cx)
            .await
            .expect_err("escape");
        assert!(matches!(err, ToolError::Denied { .. }));
    }

    #[tokio::test]
    async fn query_deserialize_returns_the_matching_path_first() {
        let tmp = tempfile::tempdir().expect("tempdir");
        lockfile(tmp.path(), LOCK);
        write_rows(
            &tmp.path().join("serde").join("1.0.210"),
            concat!(
                "{\"path\":\"serde::Deserialize\",\"kind\":\"trait\",\"docs\":\"A data structure that can be deserialized.\"}\n",
                "{\"path\":\"serde::de::value::Error\",\"kind\":\"struct\",\"docs\":\"Deserialize this value once.\"}\n",
            ),
        );
        let cx = context(tmp.path());
        let resolved = DocsResolveTool
            .call(json!({"name": "serde"}), &cx)
            .await
            .expect("resolve");
        assert_eq!(resolved.text, "cargo/serde/1.0.210");
        let out = DocsQueryTool::new(tmp.path().to_path_buf())
            .call(json!({"name": "serde", "query": "Deserialize"}), &cx)
            .await
            .expect("query");
        assert!(!out.is_error);
        assert!(
            out.text.starts_with("serde::Deserialize trait\n"),
            "{}",
            out.text
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn missing_cache_returns_not_cached_and_does_not_construct_fetch() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cx = context(tmp.path());
        let before = FETCH_CONSTRUCTED.with(Cell::get);
        let out = DocsQueryTool::new(tmp.path().to_path_buf())
            .call(
                json!({"name": "serde", "query": "Deserialize", "version": "9.9.9"}),
                &cx,
            )
            .await
            .expect("query");
        assert_eq!(out.text, "not cached");
        assert!(!out.is_error);
        assert_eq!(FETCH_CONSTRUCTED.with(Cell::get), before);
        assert_ne!(TypeId::of::<DocsQueryTool>(), TypeId::of::<DocsFetchTool>());
    }

    #[tokio::test]
    async fn query_alias_renames_library_name_and_question() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_rows(
            &tmp.path().join("serde").join("1.0.210"),
            "{\"path\":\"serde::Deserialize\",\"kind\":\"trait\",\"docs\":\"Deserialize a value.\"}\n",
        );
        let cx = context(tmp.path());
        let out = DocsQueryTool::new(tmp.path().to_path_buf())
            .call(
                json!({
                    "libraryName": "serde",
                    "question": "Deserialize",
                    "version": "1.0.210"
                }),
                &cx,
            )
            .await
            .expect("alias");
        assert!(
            out.text.starts_with("serde::Deserialize trait\n"),
            "{}",
            out.text
        );
    }

    #[tokio::test]
    async fn query_caps_hits_at_five_and_excerpts_at_400_chars() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let docs = format!("widget {}", "x".repeat(500));
        let mut rows = String::new();
        for i in 0..6 {
            rows.push_str(&format!(
                "{}\n",
                serde_json::json!({"path": format!("p{i}"), "kind": "struct", "docs": docs})
            ));
        }
        write_rows(&tmp.path().join("demo").join("1.0.0"), &rows);
        let cx = context(tmp.path());
        let out = DocsQueryTool::new(tmp.path().to_path_buf())
            .call(
                json!({"name": "demo", "query": "widget", "version": "1.0.0"}),
                &cx,
            )
            .await
            .expect("query");
        assert_eq!(out.text.split("\n\n").count(), MAX_HITS);
        let excerpt = out.text.lines().nth(1).expect("excerpt");
        assert_eq!(excerpt.chars().count(), EXCERPT_CHARS);
        assert!(out.text.starts_with("p0 struct\n"));
    }

    #[tokio::test]
    async fn rtok_version_directory_wins_over_the_cox_cache() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let rtok = tmp.path().join("rtok");
        let cox = tmp.path().join("cox");
        write_rows(
            &rtok.join("serde").join("1.0.0"),
            "{\"path\":\"from_rtok\",\"kind\":\"struct\",\"docs\":\"Deserialize from rtok\"}\n",
        );
        write_rows(
            &cox.join("serde").join("1.0.0"),
            "{\"path\":\"from_cox\",\"kind\":\"struct\",\"docs\":\"Deserialize from cox\"}\n",
        );
        let cx = context(tmp.path());
        let tool = DocsQueryTool {
            cache: cox,
            rtok: Some(rtok),
        };
        let out = tool
            .call(
                json!({"name": "serde", "query": "Deserialize", "version": "1.0.0"}),
                &cx,
            )
            .await
            .expect("query");
        assert!(out.text.starts_with("from_rtok struct\n"), "{}", out.text);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn llms_txt_search_reads_headings_and_does_not_download() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::write(
            tmp.path().join("llms.txt"),
            "# Widgets\n\nDeserialize a widget here.\n\n# Other\n\nNothing.\n",
        )
        .expect("llms");
        let cx = context(tmp.path());
        let before = FETCH_CONSTRUCTED.with(Cell::get);
        let out = DocsQueryTool::new(tmp.path().to_path_buf())
            .call(json!({"name": "llms", "query": "Deserialize"}), &cx)
            .await
            .expect("llms");
        assert!(out.text.starts_with("Widgets section\n"), "{}", out.text);
        assert!(out.text.contains("Deserialize a widget"));
        assert_eq!(FETCH_CONSTRUCTED.with(Cell::get), before);
        let empty = tempfile::tempdir().expect("tempdir");
        let missing = DocsQueryTool::new(empty.path().to_path_buf())
            .call(
                json!({"name": "llms", "query": "Deserialize"}),
                &context(empty.path()),
            )
            .await
            .expect("missing llms");
        assert_eq!(missing.text, "no llms.txt in the workspace");
    }

    fn serve(status: u16, body: Vec<u8>) -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handle = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().expect("accept");
            sock.set_read_timeout(Some(Duration::from_secs(5)))
                .expect("timeout");
            let mut buf = Vec::new();
            let mut tmp = [0u8; 2048];
            loop {
                match sock.read(&mut tmp) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&tmp[..n]);
                        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let request = String::from_utf8_lossy(&buf).into_owned();
            let reason = if status == 200 { "OK" } else { "ERR" };
            let head = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(head.as_bytes());
            let _ = sock.write_all(&body);
            request
        });
        (format!("http://{addr}"), handle)
    }

    fn rustdoc_zstd() -> Vec<u8> {
        let json = r#"{"index":{"0:1":{"docs":"A data structure that can be deserialized."}},"paths":{"0:1":{"path":["serde","Deserialize"],"kind":"trait"}}}"#;
        zstd::stream::encode_all(json.as_bytes(), 0).expect("zstd")
    }

    #[test]
    fn fetch_subject_is_the_docs_rs_url() {
        let tool = DocsFetchTool::new(PathBuf::new());
        assert_eq!(
            tool.subject(&json!({"name": "serde", "version": "1.0.210"})),
            "https://docs.rs/crate/serde/1.0.210/json.zst"
        );
    }

    #[tokio::test]
    async fn fetch_uses_the_lockfile_version_not_latest_and_writes_the_cache() {
        let tmp = tempfile::tempdir().expect("tempdir");
        lockfile(tmp.path(), LOCK);
        let compressed = rustdoc_zstd();
        let (base, handle) = serve(200, compressed.clone());
        let cx = context(tmp.path());
        let cache = tmp.path().join("cache");
        let out = DocsFetchTool::for_tests(cache.clone(), base)
            .call(json!({"name": "serde", "version": "latest"}), &cx)
            .await
            .expect("fetch");
        assert_eq!(out.text, "cached cargo/serde/1.0.210 (1 items)");
        let request = handle.join().expect("server");
        assert!(
            request.contains("/crate/serde/1.0.210/json.zst"),
            "{request}"
        );
        assert!(!request.to_ascii_lowercase().contains("authorization"));
        let dir = cache.join("serde").join("1.0.210");
        assert_eq!(fs::read(dir.join("json.zst")).expect("bytes"), compressed);
        assert_eq!(
            fs::read_to_string(dir.join("json.zst.sha256")).expect("sha"),
            sha256_hex(&compressed)
        );
        let jsonl = fs::read_to_string(dir.join("items.jsonl")).expect("jsonl");
        assert!(jsonl.contains("serde::Deserialize"), "{jsonl}");
        let queried = DocsQueryTool::new(cache)
            .call(json!({"name": "serde", "query": "Deserialize"}), &cx)
            .await
            .expect("query");
        assert!(
            queried.text.starts_with("serde::Deserialize trait\n"),
            "{}",
            queried.text
        );
    }

    #[tokio::test]
    async fn fetch_uses_a_passed_version_when_the_lockfile_has_no_entry() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (base, handle) = serve(200, rustdoc_zstd());
        let cx = context(tmp.path());
        let cache = tmp.path().join("cache");
        let out = DocsFetchTool::for_tests(cache.clone(), base)
            .call(json!({"name": "serde", "version": "1.2.3"}), &cx)
            .await
            .expect("fetch");
        assert!(out.text.starts_with("cached cargo/serde/1.2.3 "));
        let request = handle.join().expect("server");
        assert!(request.contains("/crate/serde/1.2.3/json.zst"), "{request}");
        assert!(
            cache
                .join("serde")
                .join("1.2.3")
                .join("items.jsonl")
                .is_file()
        );
    }

    #[tokio::test]
    async fn fetch_refuses_latest_when_the_lockfile_has_no_entry() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cx = context(tmp.path());
        let err = DocsFetchTool::for_tests(tmp.path().to_path_buf(), "http://127.0.0.1:9".into())
            .call(json!({"name": "serde", "version": "latest"}), &cx)
            .await
            .expect_err("latest");
        assert!(
            matches!(err, ToolError::Denied { ref why } if why.contains("latest")),
            "{err}"
        );
    }

    #[tokio::test]
    async fn failed_get_does_not_delete_a_previous_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        lockfile(tmp.path(), LOCK);
        let dir = tmp.path().join("cache").join("serde").join("1.0.210");
        write_rows(
            &dir,
            "{\"path\":\"kept\",\"kind\":\"struct\",\"docs\":\"stay\"}\n",
        );
        fs::write(dir.join("json.zst"), b"previous").expect("zst");
        let previous = fs::read(dir.join("items.jsonl")).expect("items");
        let zst = fs::read(dir.join("json.zst")).expect("zst");
        let (base, handle) = serve(500, b"nope".to_vec());
        let cx = context(tmp.path());
        let err = DocsFetchTool::for_tests(tmp.path().join("cache"), base)
            .call(json!({"name": "serde", "version": "1.0.210"}), &cx)
            .await
            .expect_err("status");
        assert!(matches!(err, ToolError::Denied { .. }), "{err}");
        let _ = handle.join();
        assert_eq!(fs::read(dir.join("items.jsonl")).expect("items"), previous);
        assert_eq!(fs::read(dir.join("json.zst")).expect("zst"), zst);

        let (base, handle) = serve(200, b"not-zstd".to_vec());
        let err = DocsFetchTool::for_tests(tmp.path().join("cache"), base)
            .call(json!({"name": "serde", "version": "1.0.210"}), &cx)
            .await
            .expect_err("decode");
        assert!(matches!(err, ToolError::Io), "{err}");
        let _ = handle.join();
        assert_eq!(fs::read(dir.join("items.jsonl")).expect("items"), previous);
        assert_eq!(fs::read(dir.join("json.zst")).expect("zst"), zst);
    }
}
