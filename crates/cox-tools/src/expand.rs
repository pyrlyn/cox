// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `expand`: retrieves archived tool output by id, with optional line bounds.
//! It is separate from archive storage so all access remains behind `Archive`.
//! `cox expand` (the CLI) shares `parse_range`/`select_lines` so both read
//! the same `lines=` grammar.

use async_trait::async_trait;
use cox_protocol::{ArchiveId, Concurrency, Risk, Tool, ToolCx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};

/// Reads a bounded selection from a prior archived tool result. The loop
/// archives and truncates this tool's output like any other, so an
/// unbounded `expand` still yields a pointer, never an unbounded read.
pub struct ExpandTool;

#[async_trait]
impl Tool for ExpandTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "expand".into(),
            description: "Retrieve archived tool output by archive id; optionally select a 1-based inclusive line range (`lines`: \"START-END\").".into(),
            input_schema: json!({"type":"object","required":["id"],"properties":{"id":{"type":"string"},"lines":{"type":"string"}}}),
            deferred: false,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }
    fn subject(&self, input: &Value) -> String {
        input
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into()
    }
    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let id: ArchiveId = self
            .subject(&input)
            .parse()
            .map_err(|_| ToolError::Denied {
                why: "invalid archive id".into(),
            })?;
        let bytes = cx.archive.get(&id).await.map_err(|_| ToolError::NotFound)?;
        let range = input
            .get("lines")
            .and_then(Value::as_str)
            .and_then(parse_range);
        Ok(ToolOutput {
            text: select_lines(&String::from_utf8_lossy(&bytes), range),
            is_error: false,
            diff: None,
            structured: None,
        })
    }
}

/// Parses `"START-END"` (1-based, inclusive); `None` when malformed or empty.
pub fn parse_range(raw: &str) -> Option<(usize, usize)> {
    let (start, end) = raw.split_once('-')?;
    let (start, end) = (start.trim().parse().ok()?, end.trim().parse().ok()?);
    (start > 0 && start <= end).then_some((start, end))
}

/// The whole text, or the numbered lines inside `range`.
pub fn select_lines(text: &str, range: Option<(usize, usize)>) -> String {
    let Some((start, end)) = range else {
        return text.to_owned();
    };
    text.lines()
        .enumerate()
        .map(|(i, line)| (i + 1, line))
        .filter(|(n, _)| (start..=end).contains(n))
        .map(|(n, line)| format!("{n}\t{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use cox_protocol::{
        Archive, ArchivePut, CallId, SandboxMode, SandboxPolicy, SessionId, StoreError,
    };
    use tokio_util::sync::CancellationToken;

    use super::*;

    #[derive(Default)]
    struct MapArchive(Mutex<HashMap<ArchiveId, Vec<u8>>>);

    #[async_trait]
    impl Archive for MapArchive {
        async fn put(&self, put: ArchivePut) -> Result<ArchiveId, StoreError> {
            let id = ArchiveId::new();
            self.0.lock().unwrap().insert(id, put.bytes);
            Ok(id)
        }
        async fn get(&self, id: &ArchiveId) -> Result<Vec<u8>, StoreError> {
            self.0
                .lock()
                .unwrap()
                .get(id)
                .cloned()
                .ok_or(StoreError::NotFound)
        }
    }

    fn cx(archive: Arc<dyn Archive>) -> ToolCx {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        crate::tool_cx(
            vec![PathBuf::from("/tmp")],
            PathBuf::from("/tmp"),
            SandboxPolicy {
                mode: SandboxMode::ReadOnly,
                network: false,
                writable: vec![],
                readonly_in_workspace: vec![],
                linux_backend: Default::default(),
            },
            archive,
            CancellationToken::new(),
            tx,
            SessionId::new(),
            CallId::new(),
        )
    }

    #[tokio::test]
    async fn expand_returns_archived_text_and_line_ranges() {
        let archive = Arc::new(MapArchive::default());
        let id = archive
            .put(ArchivePut {
                session: SessionId::new(),
                call: CallId::new(),
                tool: "echo".into(),
                subject: None,
                bytes: b"a\nb\nc\nd".to_vec(),
            })
            .await
            .unwrap();
        let cx = cx(archive);
        let all = ExpandTool
            .call(json!({"id": id.to_string()}), &cx)
            .await
            .unwrap();
        assert_eq!(all.text, "a\nb\nc\nd");
        let some = ExpandTool
            .call(json!({"id": id.to_string(), "lines": "2-3"}), &cx)
            .await
            .unwrap();
        assert_eq!(some.text, "2\tb\n3\tc");
    }

    #[tokio::test]
    async fn expand_rejects_bad_and_unknown_ids() {
        let cx = cx(Arc::new(MapArchive::default()));
        assert!(matches!(
            ExpandTool.call(json!({"id": "nope"}), &cx).await,
            Err(ToolError::Denied { .. })
        ));
        let missing = ArchiveId::new().to_string();
        assert!(matches!(
            ExpandTool.call(json!({"id": missing}), &cx).await,
            Err(ToolError::NotFound)
        ));
    }

    #[test]
    fn expand_parse_range_rejects_inverted_and_zero() {
        assert_eq!(parse_range("3-5"), Some((3, 5)));
        assert_eq!(parse_range("5-3"), None);
        assert_eq!(parse_range("0-3"), None);
        assert_eq!(parse_range("x"), None);
    }
}
