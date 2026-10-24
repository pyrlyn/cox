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
use cox_protocol::{ArchivePut, Concurrency, Risk, Tool, ToolCx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};

use crate::write::str_field;

pub struct WebFetchTool {
    http: cox_web::Client,
    /// The tool-level half of the private-address guard: the first URL's
    /// IP-literal host skips both the resolver and the redirect policy, so
    /// `new()` checks it here (T62.5). Test clients turn it off.
    private_guard: bool,
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
            private_guard: true,
        }
    }

    /// A tool over a caller-supplied client — tests serve their fixtures
    /// from loopback, which the private-address guard refuses.
    pub fn with_client(http: cox_web::Client) -> Self {
        Self {
            http,
            private_guard: false,
        }
    }
}

/// The most a model may ask one fetch to download. The body is held in
/// memory, so an unbounded `max_bytes` let one call pull gigabytes.
const MAX_BYTES: usize = 10 * 1024 * 1024;

/// Most readable text one fetch puts in front of the model (~5k tokens).
/// The download cap (`max_bytes`) bounds raw HTML; this bounds what is left
/// after reduction. Longer text is archived whole and cut here, and the
/// trailer names the archive id for `expand`, the same contract as `grep`.
pub(crate) const VISIBLE_TEXT_BYTES: usize = 20 * 1024;

/// `text` cut at the last whole line inside `limit` bytes, or `None` when it
/// already fits.
pub(crate) fn clip(text: &str, limit: usize) -> Option<&str> {
    if text.len() <= limit {
        return None;
    }
    let mut boundary = limit;
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    Some(&text[..text[..boundary].rfind('\n').unwrap_or(boundary)])
}

/// The model's `max_bytes`, defaulted and held to `1..=MAX_BYTES`.
fn max_bytes(input: &Value) -> usize {
    input
        .get("max_bytes")
        .and_then(Value::as_u64)
        .map_or(cox_web::DEFAULT_MAX_BYTES, |n| {
            usize::try_from(n).unwrap_or(usize::MAX).clamp(1, MAX_BYTES)
        })
}

#[async_trait]
impl Tool for WebFetchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "web_fetch".to_string(),
            description: "Fetch a URL and return its readable text: HTML is reduced to \
                headings, paragraphs, lists and code blocks; other content types come back \
                as-is. Pass `max_bytes` to cap what is downloaded (default 100 KiB, at \
                most 10 MiB). Text over 20 KiB is cut and archived; read the rest with \
                `expand`. http(s) only; 10 s timeout."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string"},
                    "max_bytes": {"type": "integer", "minimum": 1, "maximum": MAX_BYTES}
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
        if self.private_guard
            && let Some(why) = cox_web::refused_literal(&url)
        {
            return Err(ToolError::Denied { why: why.into() });
        }
        let max_bytes = max_bytes(&input);
        let fetched = cox_web::fetch(&self.http, &url, max_bytes, &cx.cancel).await?;
        let cut = if fetched.truncated {
            format!("; cut at max_bytes={max_bytes}")
        } else {
            String::new()
        };
        let body = match clip(&fetched.text, VISIBLE_TEXT_BYTES) {
            None => fetched.text.clone(),
            Some(head) => {
                let id = cx
                    .archive
                    .put(ArchivePut {
                        session: cx.session,
                        call: cx.call,
                        tool: "web_fetch".into(),
                        subject: Some(url.clone()),
                        bytes: fetched.text.clone().into_bytes(),
                    })
                    .await
                    .map_err(|_| ToolError::Io)?;
                format!(
                    "{head}\n… {} more bytes; archived as {id} (use `expand`)",
                    fetched.text.len() - head.len()
                )
            }
        };
        Ok(ToolOutput {
            text: format!("{body}\n[{url} · {} bytes{cut}]", fetched.bytes),
            is_error: false,
            diff: None,
            structured: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_bytes_defaults_and_is_held_to_the_cap() {
        assert_eq!(max_bytes(&json!({})), cox_web::DEFAULT_MAX_BYTES);
        assert_eq!(max_bytes(&json!({"max_bytes": 4096})), 4096);
        assert_eq!(max_bytes(&json!({"max_bytes": 0})), 1);
        assert_eq!(max_bytes(&json!({"max_bytes": u64::MAX})), MAX_BYTES);
    }

    #[test]
    fn clip_keeps_short_text_and_cuts_long_text_to_the_visible_budget() {
        assert_eq!(clip("short\npage", VISIBLE_TEXT_BYTES), None);
        // A fixed 100 KiB page: 2048 lines of 50 bytes.
        let page = format!("{}\n", "x".repeat(49)).repeat(2048);
        let head = clip(&page, VISIBLE_TEXT_BYTES).expect("cut");
        assert!(head.len() <= VISIBLE_TEXT_BYTES);
        assert!(
            head.len() > VISIBLE_TEXT_BYTES - 50,
            "cut at a line, not earlier"
        );
        // Savings: at most a fifth of the page reaches the model.
        assert!(head.len() * 5 <= page.len());
        // Multi-byte text never splits a char.
        let wide = "é".repeat(VISIBLE_TEXT_BYTES);
        assert!(clip(&wide, VISIBLE_TEXT_BYTES).is_some());
    }
}
