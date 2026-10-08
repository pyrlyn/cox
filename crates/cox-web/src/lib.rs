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

/// A client configured with cox's timeout, redirect limit, user agent and
/// the private-address guard: every connection — including each redirect
/// hop — resolves through [`PublicOnly`], which refuses loopback,
/// link-local, private and unspecified addresses (T62.5).
pub fn client() -> Client {
    match reqwest::Client::builder()
        .timeout(TIMEOUT)
        .redirect(private_guard_policy())
        .user_agent("cox (+https://github.com/pyrlyn/cox)")
        .dns_resolver(std::sync::Arc::new(PublicOnly))
        .build()
    {
        Ok(built) => built,
        // The builder only fails on a broken TLS backend; a bare client
        // still fetches, just without the timeout, and `fetch` bounds the
        // body itself. The guard is re-installed so it never depends on the
        // backend's health.
        Err(_) => reqwest::Client::builder()
            .redirect(private_guard_policy())
            .dns_resolver(std::sync::Arc::new(PublicOnly))
            .build()
            .unwrap_or_default(),
    }
}

const PRIVATE_REFUSAL: &str =
    "cox refuses to fetch loopback, link-local, private or unspecified addresses";

/// Whether `url`'s host is an IP literal the private-address guard
/// refuses, and the message saying so. The resolver and the redirect
/// policy cannot see the first URL's literal host, so the tool layer asks
/// this before its first request (T62.5).
pub fn refused_literal(url: &str) -> Option<&'static str> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    let ip: std::net::IpAddr = host.parse().ok()?;
    (!is_public(ip)).then_some(PRIVATE_REFUSAL)
}

/// A client without the private-address guard, for tests that serve their
/// fixtures from loopback. Production code always goes through [`client`].
#[doc(hidden)]
pub fn client_for_tests() -> Client {
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent("cox (+https://github.com/pyrlyn/cox)")
        .build()
        .unwrap_or_default()
}

/// The redirect half of the private-address guard (T62.5): a host name
/// re-resolves through [`PublicOnly`] on every hop, but an IP-literal host
/// skips resolution entirely, so each redirect is checked here too. The
/// custom policy also carries the five-hop limit `limited(5)` used to.
fn private_guard_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt
            .url()
            .host_str()
            .and_then(|host| host.parse::<std::net::IpAddr>().ok())
            .is_some_and(|ip| !is_public(ip))
        {
            return attempt.error(PRIVATE_REFUSAL);
        }
        if attempt.previous().len() >= 5 {
            attempt.stop()
        } else {
            attempt.follow()
        }
    })
}

/// A resolver that answers public addresses only (T62.5). `web_fetch` is
/// auto-allowed as read-only, so without this the model could read
/// `169.254.169.254`, an internal `localhost` service or an RFC1918 host
/// and see the body. Redirects re-resolve through the same resolver, so a
/// public URL that bounces to a private one is stopped too.
#[derive(Clone, Default)]
struct PublicOnly;

impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            // getaddrinfo blocks, so it runs on the blocking pool the way
            // reqwest's own resolver runs it.
            let resolved = tokio::task::spawn_blocking(move || {
                use std::net::ToSocketAddrs;
                (host.as_str(), 0)
                    .to_socket_addrs()
                    .map(|a| a.collect::<Vec<_>>())
            })
            .await
            .map_err(boxed_error)
            .and_then(|r| r.map_err(boxed_error));
            let addrs: Vec<std::net::SocketAddr> = resolved?;
            let public: Vec<_> = addrs
                .into_iter()
                .filter(|addr| is_public(addr.ip()))
                .collect();
            if public.is_empty() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "cox refuses to fetch loopback, link-local, private or unspecified addresses",
                ))
                    as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(public.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

fn boxed_error<E: std::error::Error + Send + Sync + 'static>(
    e: E,
) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(e)
}

/// Whether `ip` may be fetched: everything a browser's private-network
/// guard blocks — loopback, link-local, private (RFC1918 and IPv6 ULA),
/// unspecified and broadcast — is refused, mapped IPv4 included.
fn is_public(ip: std::net::IpAddr) -> bool {
    fn public_v4(v4: std::net::Ipv4Addr) -> bool {
        !(v4.is_private()
            || v4.is_loopback()
            || v4.is_link_local()
            || v4.is_broadcast()
            || v4.is_unspecified())
    }
    match ip {
        std::net::IpAddr::V4(v4) => public_v4(v4),
        std::net::IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return public_v4(mapped);
            }
            let first = v6.segments()[0];
            let unique_local = (first & 0xfe00) == 0xfc00;
            let link_local = (first & 0xffc0) == 0xfe80;
            !(v6.is_loopback() || v6.is_unspecified() || unique_local || link_local)
        }
    }
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
        return ToolError::Timeout;
    }
    // The private-address guard refuses before any bytes move, and reqwest
    // buries its words in the source chain; they are the useful ones.
    let mut source = std::error::Error::source(&e);
    while let Some(err) = source {
        if let Some(io) = err
            .downcast_ref::<std::io::Error>()
            .filter(|io| io.kind() == std::io::ErrorKind::PermissionDenied)
        {
            return ToolError::Denied {
                why: io.to_string(),
            };
        }
        source = err.source();
    }
    ToolError::Denied {
        why: format!("fetch failed: {e}"),
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
    fn private_addresses_are_not_public() {
        let refused = [
            "127.0.0.1",
            "0.0.0.0",
            "169.254.169.254",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "::1",
            "::",
            "fe80::1",
            "fc00::1",
            "fd12:3456::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
        ];
        let allowed = ["1.1.1.1", "93.184.216.34", "2606:4700::1111"];
        for raw in refused {
            let ip: std::net::IpAddr = raw.parse().expect(raw);
            assert!(!is_public(ip), "{raw} must be refused");
        }
        for raw in allowed {
            let ip: std::net::IpAddr = raw.parse().expect(raw);
            assert!(is_public(ip), "{raw} must be allowed");
        }
    }

    /// T62.5: `web_fetch` is auto-allowed read-only, so a private target
    /// must be refused before any bytes move — by name or literal, loopback
    /// included.
    #[test]
    fn fetching_a_private_target_is_refused() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let url = "http://localhost:1/";
        let fetched = runtime.block_on(fetch(&client(), url, DEFAULT_MAX_BYTES, &cancel));
        match fetched {
            Err(ToolError::Denied { why }) => {
                assert!(why.contains("refuses"), "{url}: {why}");
            }
            Ok(_) => panic!("{url} must be denied, but the fetch succeeded"),
            Err(other) => panic!("{url} must be denied, got {other:?}"),
        }
    }

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
