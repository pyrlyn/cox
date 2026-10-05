// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `web_fetch`: `Tool` glue over the `cox-web` fetch engine (plan.md T3.8,
//! §1.11). Owns `ToolCx`, input-JSON parsing and tool-output framing; the
//! HTTP GET and the HTML→text reduction moved to `cox-web` (T32.7) because
//! `reqwest` belongs to the crate that opens the socket, not to every
//! consumer of `cox-tools`. The permission engine's `WebFetch(domain:…)`
//! rules still match on the URL `subject` reports.

use async_trait::async_trait;
use cox_protocol::{Concurrency, Risk, Tool, ToolCx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};

use crate::write::str_field;

pub struct WebFetchTool {
    http: cox_web::Client,
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebFetchTool {
    pub fn new() -> Self {
        Self {
            http: cox_web::client(),
        }
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "web_fetch".to_string(),
            description: "Fetch a URL and return its readable text: HTML is reduced to \
                headings, paragraphs, lists and code blocks; other content types come back \
                as-is. Pass `max_bytes` to cap what is downloaded (default 100 KiB); the \
                result says when it was cut. Only http and https URLs; 10 s timeout."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string"},
                    "max_bytes": {"type": "integer", "minimum": 1}
                },
                "required": ["url"]
            }),
            deferred: true,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }

    fn subject(&self, input: &Value) -> String {
        input
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let url = str_field(&input, "url")?;
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(ToolError::Denied {
                why: format!("only http(s) URLs can be fetched, got {url:?}"),
            });
        }
        let max_bytes = input
            .get("max_bytes")
            .and_then(Value::as_u64)
            .map_or(cox_web::DEFAULT_MAX_BYTES, |n| n as usize);
        let fetched = cox_web::fetch(&self.http, &url, max_bytes, &cx.cancel).await?;
        let cut = if fetched.truncated {
            format!("; cut at max_bytes={max_bytes}")
        } else {
            String::new()
        };
        Ok(ToolOutput {
            text: format!("{}\n[{url} · {} bytes{cut}]", fetched.text, fetched.bytes),
            is_error: false,
            diff: None,
            structured: None,
        })
    }
}
