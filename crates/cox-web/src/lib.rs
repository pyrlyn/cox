// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The web-fetch engine: an HTTP GET with a hard timeout and a byte cap,
//! plus a hand-rolled HTML→text reduction (headings, paragraphs, lists and
//! code) — a tag walk rather than a DOM crate. Split out of `cox-tools`
//! (T32.7) because `reqwest` is the dependency this crate exists to
//! isolate. The `web_fetch` `Tool` impl stays in `cox_tools::web_fetch`:
//! `ToolCx`, input-JSON parsing and tool-output framing are cox-tools
//! concerns this engine does not need; it calls [`client`] and [`fetch`]
//! instead. [`Client`] is re-exported so cox-tools can name the field type
//! without depending on `reqwest` itself.

// why: this crate was not under `missing_docs` before the workspace lints;
// drop this allow once its public items are documented.
#![allow(missing_docs)]

use std::time::Duration;

use cox_protocol::ToolError;
use tokio_util::sync::CancellationToken;

pub use reqwest::Client;

/// Per-request timeout.
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// Default cap on bytes downloaded when the caller does not set `max_bytes`.
pub const DEFAULT_MAX_BYTES: usize = 100 * 1024;
/// Elements whose content is chrome, not the page.
const DROP: &[&str] = &[
    "script", "style", "noscript", "nav", "header", "footer", "aside", "svg", "template", "iframe",
    "form", "button",
];

/// A fetch's outcome: readable text, the raw byte count, and whether
/// `max_bytes` cut the body short.
pub struct Fetched {
    pub text: String,
    pub bytes: usize,
    pub truncated: bool,
}

/// A client configured with cox's timeout, redirect limit and user agent.
pub fn client() -> Client {
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent("cox (+https://github.com/pyrlyn/cox)")
        .build()
        // The builder only fails on a broken TLS backend; a bare client
        // still fetches, just without the timeout, and `fetch` bounds the
        // body itself.
        .unwrap_or_default()
}

/// Fetches `url` with `http`, capping the body at `max_bytes` and reducing
/// HTML responses to readable text. Cancels via `cancel`
/// (`Submission::Interrupt`).
pub async fn fetch(
    http: &Client,
    url: &str,
    max_bytes: usize,
    cancel: &CancellationToken,
) -> Result<Fetched, ToolError> {
    let request = http.get(url).send();
    let mut response = tokio::select! {
        _ = cancel.cancelled() => return Err(ToolError::Cancelled),
        r = request => r.map_err(fetch_error)?,
    };
    let status = response.status();
    if !status.is_success() {
        return Err(ToolError::Denied {
            why: format!("{url} answered HTTP {status}"),
        });
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut body = Vec::new();
    let mut truncated = false;
    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => return Err(ToolError::Cancelled),
            c = response.chunk() => c.map_err(fetch_error)?,
        };
        let Some(chunk) = chunk else { break };
        body.extend_from_slice(&chunk);
        if body.len() >= max_bytes {
            body.truncate(max_bytes);
            truncated = true;
            break;
        }
    }
    let raw = String::from_utf8_lossy(&body);
    let text = if content_type.contains("html") {
        extract(&raw)
    } else {
        raw.trim().to_string()
    };
    Ok(Fetched {
        bytes: body.len(),
        text,
        truncated,
    })
}

fn fetch_error(e: reqwest::Error) -> ToolError {
    if e.is_timeout() {
        ToolError::Timeout
    } else {
        ToolError::Denied {
            why: format!("fetch failed: {e}"),
        }
    }
}

/// HTML to readable text.
pub fn extract(html: &str) -> String {
    let title = between(html, "<title", "</title>")
        .map(|t| {
            decode(&t[t.find('>').map_or(0, |i| i + 1)..])
                .trim()
                .to_string()
        })
        .unwrap_or_default();
    let body = ["<main", "<article", "<body"]
        .iter()
        .find_map(|open| {
            let close = format!("</{}>", &open[1..]);
            between(html, open, &close)
        })
        .unwrap_or(html);
    let mut out = String::new();
    let mut rest = body;
    let mut in_pre = false;
    while let Some(lt) = rest.find('<') {
        push_text(&mut out, &rest[..lt], in_pre);
        rest = &rest[lt..];
        if let Some(stripped) = rest.strip_prefix("<!--") {
            rest = stripped.find("-->").map_or("", |i| &stripped[i + 3..]);
            continue;
        }
        let Some(gt) = rest.find('>') else { break };
        let tag = &rest[1..gt];
        rest = &rest[gt + 1..];
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let closing = tag.starts_with('/');
        if !closing && DROP.contains(&name.as_str()) {
            let close = format!("</{name}");
            rest = find_ci(rest, &close)
                .and_then(|i| rest[i..].find('>').map(|j| &rest[i + j + 1..]))
                .unwrap_or("");
            continue;
        }
        match name.as_str() {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                if closing {
                    out.push_str("\n\n");
                } else {
                    let level = name[1..].parse::<usize>().unwrap_or(1);
                    out.push_str("\n\n");
                    out.push_str(&"#".repeat(level));
                    out.push(' ');
                }
            }
            "p" | "div" | "section" | "blockquote" | "tr" | "ul" | "ol" | "table" | "dl" | "dd"
            | "dt" => out.push_str("\n\n"),
            "br" => out.push('\n'),
            "li" if !closing => out.push_str("\n- "),
            "td" | "th" => out.push('\t'),
            "pre" => {
                in_pre = !closing;
                if closing {
                    let end = out.trim_end_matches('\n').len();
                    out.truncate(end);
                }
                out.push_str("\n```\n");
            }
            "code" if !in_pre => out.push('`'),
            _ => {}
        }
    }
    push_text(&mut out, rest, in_pre);
    let text = tidy(&out);
    if title.is_empty() {
        text
    } else {
        format!("# {title}\n\n{text}")
    }
}

fn push_text(out: &mut String, text: &str, in_pre: bool) {
    if in_pre {
        out.push_str(&decode(text));
        return;
    }
    // Inline whitespace collapses to one space; a space survives only at
    // the boundary it appeared on (`First <b>bold</b> paragraph`).
    let leading = text.starts_with(char::is_whitespace);
    let trailing = text.ends_with(char::is_whitespace);
    let mut words = text.split_whitespace().peekable();
    if leading && !out.is_empty() && !out.ends_with([' ', '\n']) {
        out.push(' ');
    }
    let mut any = false;
    while let Some(word) = words.next() {
        any = true;
        out.push_str(&decode(word));
        if words.peek().is_some() {
            out.push(' ');
        }
    }
    if any && trailing {
        out.push(' ');
    }
}

/// The text between `open` (a tag prefix, case-insensitive) and `close`.
fn between<'a>(html: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = find_ci(html, open)?;
    let after = &html[start..];
    let end = find_ci(after, close)?;
    Some(&after[..end])
}

fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let lower = haystack.to_ascii_lowercase();
    lower.find(&needle.to_ascii_lowercase())
}

/// Collapses runs of blank lines and trailing spaces.
fn tidy(text: &str) -> String {
    let mut out = String::new();
    let mut blank = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            if blank <= 1 && !out.is_empty() {
                out.push('\n');
            }
        } else {
            blank = 0;
            out.push_str(line);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

/// The entities that matter in prose and code; the rest stay literal.
fn decode(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest.find(';').filter(|i| *i <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            e => e
                .strip_prefix('#')
                .and_then(|n| {
                    n.strip_prefix('x')
                        .or_else(|| n.strip_prefix('X'))
                        .map_or_else(|| n.parse().ok(), |h| u32::from_str_radix(h, 16).ok())
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_fetch_extract_keeps_headings_paragraphs_lists_and_code() {
        let html = r#"<html><head><title>Docs &amp; More</title><style>p{}</style></head>
<body><nav><a href="/">Home</a></nav><script>var x = 1;</script>
<article><h1>Guide</h1><p>First <b>bold</b> paragraph.</p>
<ul><li>one</li><li>two</li></ul>
<pre><code>let x = 1 &lt; 2;
</code></pre><p>Inline <code>call()</code> here.</p></article>
<footer>© 2026</footer></body></html>"#;
        let text = extract(html);
        assert_eq!(
            text,
            "# Docs & More\n\n# Guide\n\nFirst bold paragraph.\n\n- one\n- two\n\n```\nlet x = 1 < 2;\n```\n\nInline `call()` here."
        );
        assert!(!text.contains("Home") && !text.contains("var x") && !text.contains("2026"));
    }

    #[test]
    fn web_fetch_decode_handles_numeric_and_unknown_entities() {
        assert_eq!(decode("a &#65;&#x42; &unknown; &"), "a AB &unknown; &");
    }
}
