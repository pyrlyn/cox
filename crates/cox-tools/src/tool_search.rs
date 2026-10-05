// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `tool_search`: BM25 over the deferred tools' names and descriptions
//! (plan.md T3.8, D6d). The core keeps every non-core schema out of the
//! prompt until the model asks for it here; the names this returns in
//! `structured.discovered` are what the core appends to the request. Own
//! ~60-line ranking rather than a dependency: the corpus is a few dozen
//! short documents.

use async_trait::async_trait;
use cox_protocol::{Concurrency, Risk, Tool, ToolCx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};

use crate::write::str_field;

/// How many specs one search returns at most (plan.md §1.11).
const MAX_HITS: usize = 5;
const K1: f64 = 1.2;
const B: f64 = 0.75;

pub struct ToolSearchTool {
    specs: Vec<ToolSpec>,
    docs: Vec<Vec<String>>,
}

impl ToolSearchTool {
    /// Indexes the deferred specs among `specs`; the rest are already visible.
    pub fn new(specs: impl IntoIterator<Item = ToolSpec>) -> Self {
        let specs: Vec<ToolSpec> = specs.into_iter().filter(|s| s.deferred).collect();
        let docs = specs
            .iter()
            .map(|s| tokens(&format!("{} {}", s.name, s.description)))
            .collect();
        Self { specs, docs }
    }

    /// The best-matching deferred specs, most relevant first.
    pub fn search(&self, query: &str) -> Vec<&ToolSpec> {
        let query = tokens(query);
        let mut scored: Vec<(f64, usize)> = bm25(&query, &self.docs)
            .into_iter()
            .enumerate()
            .filter(|(_, score)| *score > 0.0)
            .map(|(i, score)| (score, i))
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        scored
            .into_iter()
            .take(MAX_HITS)
            .map(|(_, i)| &self.specs[i])
            .collect()
    }
}

/// Lowercased alphanumeric runs; `mcp__github__create_issue` becomes
/// `mcp github create issue`.
fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Okapi BM25 scores of every document for `query`.
fn bm25(query: &[String], docs: &[Vec<String>]) -> Vec<f64> {
    let n = docs.len() as f64;
    let avg_len = if docs.is_empty() {
        0.0
    } else {
        docs.iter().map(Vec::len).sum::<usize>() as f64 / n
    };
    docs.iter()
        .map(|doc| {
            let len = doc.len() as f64;
            query
                .iter()
                .map(|term| {
                    let tf = doc.iter().filter(|t| *t == term).count() as f64;
                    if tf == 0.0 {
                        return 0.0;
                    }
                    let df = docs.iter().filter(|d| d.contains(term)).count() as f64;
                    let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                    idf * (tf * (K1 + 1.0)) / (tf + K1 * (1.0 - B + B * len / avg_len))
                })
                .sum()
        })
        .collect()
}

#[async_trait]
impl Tool for ToolSearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "tool_search".to_string(),
            description: "Find tools that are not in your current tool list. Only the core \
                tools are always present; MCP servers, `ask_user`, `web_fetch`, `agent` and \
                other extras are found here. Pass a short `query` describing what you need \
                (\"create github issue\", \"fetch a web page\"); up to 5 matching tool \
                schemas are returned and become callable on your next turn."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"]
            }),
            deferred: false,
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
        let query = str_field(&input, "query")?;
        let hits = self.search(&query);
        let names: Vec<&str> = hits.iter().map(|s| s.name.as_str()).collect();
        let text = if hits.is_empty() {
            format!("no deferred tool matches {query:?}")
        } else {
            serde_json::to_string_pretty(&hits).map_err(|_| ToolError::Io)?
        };
        Ok(ToolOutput {
            text,
            is_error: false,
            diff: None,
            structured: Some(json!({ "discovered": names })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, description: &str, deferred: bool) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: description.into(),
            input_schema: json!({"type": "object"}),
            deferred,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }

    fn index() -> ToolSearchTool {
        ToolSearchTool::new([
            spec("read", "read a file", false),
            spec(
                "mcp__github__create_issue",
                "Create a GitHub issue in a repository",
                true,
            ),
            spec("mcp__github__list_issues", "List GitHub issues", true),
            spec("web_fetch", "Fetch a web page as readable text", true),
            spec("ask_user", "Ask the user a question", true),
            spec(
                "mcp__db__query",
                "Run a SQL query against the database",
                true,
            ),
            spec("mcp__db__tables", "List database tables", true),
            spec("mcp__slack__post", "Post a Slack message", true),
        ])
    }

    #[test]
    fn tool_search_ranks_the_matching_deferred_tool_first() {
        let idx = index();
        let hits = idx.search("create a github issue");
        assert_eq!(hits[0].name, "mcp__github__create_issue");
        assert!(
            hits.iter().all(|s| s.deferred),
            "core tools are never returned"
        );
        assert_eq!(idx.search("fetch web page")[0].name, "web_fetch");
    }

    #[test]
    fn tool_search_returns_at_most_five_and_nothing_for_no_match() {
        let idx = ToolSearchTool::new((0..9).map(|i| spec(&format!("t{i}"), "widget maker", true)));
        assert_eq!(idx.search("widget").len(), MAX_HITS);
        let none = index();
        assert!(none.search("kubernetes").is_empty());
    }

    #[tokio::test]
    async fn tool_search_reports_discovered_names_in_structured_output() {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let cx = crate::tool_cx(
            vec![],
            std::path::PathBuf::from("/tmp"),
            cox_protocol::SandboxPolicy {
                mode: cox_protocol::SandboxMode::ReadOnly,
                network: false,
                writable: vec![],
                readonly_in_workspace: vec![],
                linux_backend: Default::default(),
            },
            std::sync::Arc::new(NoopArchive),
            tokio_util::sync::CancellationToken::new(),
            tx,
            cox_protocol::SessionId::new(),
            cox_protocol::CallId::new(),
        );
        let out = index()
            .call(json!({"query": "slack"}), &cx)
            .await
            .expect("search");
        assert_eq!(
            out.structured,
            Some(json!({"discovered": ["mcp__slack__post"]}))
        );
        assert!(out.text.contains("\"name\": \"mcp__slack__post\""));
    }

    struct NoopArchive;

    #[async_trait]
    impl cox_protocol::Archive for NoopArchive {
        async fn put(
            &self,
            _put: cox_protocol::ArchivePut,
        ) -> Result<cox_protocol::ArchiveId, cox_protocol::StoreError> {
            Ok(cox_protocol::ArchiveId::new())
        }
        async fn get(
            &self,
            _id: &cox_protocol::ArchiveId,
        ) -> Result<Vec<u8>, cox_protocol::StoreError> {
            Ok(Vec::new())
        }
    }
}
