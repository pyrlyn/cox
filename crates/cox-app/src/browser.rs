// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The agent's browser (T51.7, DT§3.3): `browser_open`, `browser_read` and
//! `browser_screenshot`, three deferred tools over a [`Browser`] the host
//! supplies (the macOS app's `WebPage`). Here rather than in `cox-tools`
//! because only a surface with a page to drive has one: a session gets
//! these tools at open only when [`crate::app::Host::browser`] returns a
//! browser, so the tool set stays byte-stable within a session.
//!
//! Trust: `browser_open` passes only an `http`/`https` URL, and its risk is
//! per call, so the permission engine decides it like `web_fetch` with the
//! URL as the subject; the page's text is untrusted and passes the
//! terminal-text guard before the model sees it; its full text is what the
//! core archives before it shortens it (the lossless rule).

use std::sync::Arc;

use async_trait::async_trait;
use cox_protocol::image;
use cox_protocol::{Concurrency, Risk, Tool, ToolCx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};
use url::{Host, Url};

/// What `browser_read` reports of the page on show.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PageText {
    pub title: String,
    pub url: String,
    /// The page's visible text (`document.body.innerText`).
    pub text: String,
}

/// Why the host's browser could not do what a tool asked.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BrowserError {
    /// No page has been opened yet.
    #[error("no page is open; call browser_open first")]
    NoPage,
    /// The page did not load, or could not be read or captured.
    #[error("{0}")]
    Page(String),
}

/// The page the host drives for the agent. Plain Rust, so a test fakes it;
/// `cox-ffi` adapts the Swift one (T51.8).
#[async_trait]
pub trait Browser: Send + Sync {
    /// Loads `url`, already checked to be `http`/`https`.
    async fn load(&self, url: &str) -> Result<(), BrowserError>;
    /// The page's title, address and visible text.
    async fn text(&self) -> Result<PageText, BrowserError>;
    /// The visible page as an image (PNG).
    async fn snapshot(&self) -> Result<Vec<u8>, BrowserError>;
}

/// The three browser tools over `browser`, or none without one.
pub fn browser_tools(browser: Option<Arc<dyn Browser>>) -> Vec<Arc<dyn Tool>> {
    let Some(browser) = browser else {
        return Vec::new();
    };
    vec![
        Arc::new(BrowserOpenTool(Arc::clone(&browser))),
        Arc::new(BrowserReadTool(Arc::clone(&browser))),
        Arc::new(BrowserScreenshotTool(browser)),
    ]
}

/// `url`, if it parses and is `http`/`https`.
fn web_url(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|e| format!("{raw:?} is not a URL: {e}"))?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        other => Err(format!("only http and https pages open, not {other}:")),
    }
}

/// T51.10: what the person typed in the browser pane's address field, as
/// the URL to load, or `None` when it is not a web page. The same rule as
/// `browser_open`. A bare host gets a scheme the way a browser's field adds
/// one: `http` for this machine's dev servers, `https` for the rest.
pub fn web_address(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if text.contains("://") {
        return web_url(text).ok().map(String::from);
    }
    let local = web_url(&format!("http://{text}")).ok().filter(is_loopback);
    let url = local.map_or_else(|| web_url(&format!("https://{text}")), Ok);
    url.ok().map(String::from)
}

/// Whether `url` stays on this machine: `localhost` (and its subdomains,
/// RFC 6761) or a loopback address.
fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(d)) => {
            let d = d.to_ascii_lowercase();
            d == "localhost" || d.ends_with(".localhost")
        }
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

fn url_field(input: &Value) -> &str {
    input.get("url").and_then(Value::as_str).unwrap_or_default()
}

/// A failed browser step as a result the model reads, not a tool error:
/// the page is the host's, and the model may try another address.
fn failed(e: &BrowserError) -> ToolOutput {
    ToolOutput {
        text: e.to_string(),
        is_error: true,
        diff: None,
        structured: None,
    }
}

fn spec(name: &str, description: &str, input_schema: Value, risk: Risk) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: description.into(),
        input_schema,
        deferred: true,
        risk,
        // One page for every call: a read must see the page the open before
        // it loaded.
        concurrency: Concurrency::Exclusive,
    }
}

fn no_input() -> Value {
    json!({ "type": "object", "properties": {} })
}

/// `browser_open { url }`: loads a page in the host's browser.
pub struct BrowserOpenTool(Arc<dyn Browser>);

#[async_trait]
impl Tool for BrowserOpenTool {
    fn spec(&self) -> ToolSpec {
        let schema = json!({
            "type": "object",
            "properties": { "url": { "type": "string" } },
            "required": ["url"]
        });
        // The class of a remote page; `risk` lowers it for a loopback one.
        spec(
            "browser_open",
            "Open a web page in the app's browser pane, which the person sees. \
             Only http and https URLs. Then `browser_read` for its text or \
             `browser_screenshot` for how it looks.",
            schema,
            Risk::Write,
        )
    }

    fn subject(&self, input: &Value) -> String {
        url_field(input).to_string()
    }

    /// A loopback page (the project's dev server) reads like a file; any
    /// other host reaches the network. There is no `Network` risk class, so
    /// that is `Write`: plan mode refuses it, the default mode asks, and a
    /// `browser_open(domain:…)` rule allows or denies it by host.
    fn risk(&self, input: &Value) -> Risk {
        match web_url(url_field(input)) {
            Ok(url) if is_loopback(&url) => Risk::ReadOnly,
            _ => Risk::Write,
        }
    }

    async fn call(&self, input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let url = web_url(url_field(&input)).map_err(|why| ToolError::Denied { why })?;
        if let Err(e) = self.0.load(url.as_str()).await {
            return Ok(failed(&e));
        }
        Ok(ToolOutput {
            text: format!("opened {url}"),
            is_error: false,
            diff: None,
            structured: None,
        })
    }
}

/// `browser_read {}`: the open page's title, address and visible text.
pub struct BrowserReadTool(Arc<dyn Browser>);

#[async_trait]
impl Tool for BrowserReadTool {
    fn spec(&self) -> ToolSpec {
        spec(
            "browser_read",
            "The title, URL and visible text of the page open in the browser pane.",
            no_input(),
            Risk::ReadOnly,
        )
    }

    fn subject(&self, _input: &Value) -> String {
        String::new()
    }

    async fn call(&self, _input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let page = match self.0.text().await {
            Ok(page) => page,
            Err(e) => return Ok(failed(&e)),
        };
        // Someone else wrote every byte of it, the title and address too.
        // The whole text goes back: the core archives it before it cuts
        // what the model sees to the tool cap.
        let text = format!(
            "Title: {}\nURL: {}\n\n{}",
            cox_sanitize::sanitize(&page.title),
            cox_sanitize::sanitize(&page.url),
            cox_sanitize::sanitize(&page.text),
        );
        Ok(ToolOutput {
            text,
            is_error: false,
            diff: None,
            structured: None,
        })
    }
}

/// `browser_screenshot {}`: the open page as an image the model sees.
pub struct BrowserScreenshotTool(Arc<dyn Browser>);

#[async_trait]
impl Tool for BrowserScreenshotTool {
    fn spec(&self) -> ToolSpec {
        spec(
            "browser_screenshot",
            "A screenshot of the page open in the browser pane.",
            no_input(),
            Risk::ReadOnly,
        )
    }

    fn subject(&self, _input: &Value) -> String {
        String::new()
    }

    /// The bytes go through P40's one image check: sniffed, capped, then the
    /// tool-image payload the core forwards to the model (T40.5).
    async fn call(&self, _input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let bytes = match self.0.snapshot().await {
            Ok(bytes) => bytes,
            Err(e) => return Ok(failed(&e)),
        };
        if bytes.len() > image::MAX_IMAGE_BYTES {
            return Err(ToolError::TooLarge {
                bytes: bytes.len() as u64,
                cap: image::MAX_IMAGE_BYTES as u64,
            });
        }
        let Some(media_type) = image::sniff(&bytes) else {
            return Ok(failed(&BrowserError::Page(
                "the screenshot is not a PNG, JPEG, GIF or WebP image".into(),
            )));
        };
        Ok(ToolOutput {
            text: format!("screenshot, {media_type}, {} bytes", bytes.len()),
            is_error: false,
            diff: None,
            structured: Some(image::to_structured(media_type, &bytes)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cox_core::permission::{Engine, Outcome};
    use cox_protocol::config::PermissionsConfig;
    use cox_protocol::ids::{CallId, SessionId};
    use cox_protocol::types::{ApprovalPolicy, PermissionMode, SandboxMode, ToolCall};
    use cox_protocol::{Archive, ArchiveId, ArchivePut, SandboxPolicy, StoreError};
    use std::path::Path;
    use std::sync::Mutex;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    /// A page in memory that remembers what it was asked to load.
    #[derive(Default)]
    pub(crate) struct FakeBrowser {
        pub(crate) page: PageText,
        pub(crate) shot: Vec<u8>,
        pub(crate) loaded: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl Browser for FakeBrowser {
        async fn load(&self, url: &str) -> Result<(), BrowserError> {
            self.loaded.lock().expect("loaded").push(url.into());
            Ok(())
        }
        async fn text(&self) -> Result<PageText, BrowserError> {
            Ok(self.page.clone())
        }
        async fn snapshot(&self) -> Result<Vec<u8>, BrowserError> {
            Ok(self.shot.clone())
        }
    }

    struct NoArchive;

    #[async_trait]
    impl Archive for NoArchive {
        async fn put(&self, _: ArchivePut) -> Result<ArchiveId, StoreError> {
            Ok(ArchiveId::new())
        }
        async fn get(&self, _: &ArchiveId) -> Result<Vec<u8>, StoreError> {
            Ok(Vec::new())
        }
    }

    fn cx() -> ToolCx {
        let dir = std::env::temp_dir();
        let (output, _) = tokio::sync::mpsc::channel(1);
        cox_tools::tool_cx(
            vec![dir.clone()],
            dir,
            SandboxPolicy {
                mode: SandboxMode::ReadOnly,
                network: false,
                writable: vec![],
                readonly_in_workspace: vec![],
                linux_backend: Default::default(),
            },
            Arc::new(NoArchive),
            Default::default(),
            output,
            SessionId::new(),
            CallId::new(),
        )
    }

    fn tool(browser: &Arc<FakeBrowser>, name: &str) -> Arc<dyn Tool> {
        let browser: Arc<dyn Browser> = browser.clone();
        let tools = browser_tools(Some(browser));
        let found = tools.into_iter().find(|t| t.spec().name == name);
        found.expect("the tool is registered")
    }

    #[test]
    fn browser_tools_absent_without_a_host_browser() {
        assert!(browser_tools(None).is_empty());
        let browser: Arc<dyn Browser> = Arc::new(FakeBrowser::default());
        let tools = browser_tools(Some(browser));
        let names: Vec<String> = tools.iter().map(|t| t.spec().name).collect();
        assert_eq!(
            names,
            ["browser_open", "browser_read", "browser_screenshot"]
        );
        // Found through `tool_search`: the core tools and the cache prefix
        // stay what they were without a browser.
        assert!(tools.iter().all(|t| t.spec().deferred));
    }

    #[tokio::test]
    async fn browser_open_refuses_non_http_schemes() {
        let browser = Arc::new(FakeBrowser::default());
        let open = tool(&browser, "browser_open");
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,hi",
            "about:blank",
            "not a url",
        ] {
            let result = open.call(json!({ "url": url }), &cx()).await;
            assert!(
                matches!(result, Err(ToolError::Denied { .. })),
                "{url}: {result:?}"
            );
        }
        assert!(browser.loaded.lock().expect("loaded").is_empty());
        let https = json!({ "url": "https://example.com/a" });
        let out = open.call(https, &cx()).await.expect("an https page opens");
        assert!(!out.is_error);
        let loaded = browser.loaded.lock().expect("loaded").clone();
        assert_eq!(loaded, ["https://example.com/a"]);
    }

    #[test]
    fn browser_open_to_a_remote_host_is_network_risk() {
        let open = tool(&Arc::new(FakeBrowser::default()), "browser_open");
        let risk = |url: &str| open.risk(&json!({ "url": url }));
        // No `Risk::Network` exists: a remote host is `Write`, which plan
        // mode refuses and the default mode asks about.
        assert_eq!(risk("https://example.com"), Risk::Write);
        assert_eq!(risk("http://localhost.example.com"), Risk::Write);
        assert_eq!(risk("http://10.0.0.1:8080"), Risk::Write);
        assert_eq!(risk("file:///etc/passwd"), Risk::Write);
        assert_eq!(risk("http://localhost:3000"), Risk::ReadOnly);
        assert_eq!(risk("http://app.localhost"), Risk::ReadOnly);
        assert_eq!(risk("http://127.0.0.1:8080/x"), Risk::ReadOnly);
        assert_eq!(risk("http://[::1]:5173"), Risk::ReadOnly);
        // Rules match on the URL, as `WebFetch(domain:…)` does.
        let subject = open.subject(&json!({ "url": "https://example.com/a" }));
        assert_eq!(subject, "https://example.com/a");

        let permissions = PermissionsConfig {
            deny: vec!["browser_open(domain:evil.test)".into()],
            ..PermissionsConfig::default()
        };
        let engine = Engine::compile(&permissions, None, Path::new("/repo")).expect("rules");
        let decide = |url: &str, mode: PermissionMode| {
            let input = json!({ "url": url });
            let call = ToolCall {
                id: CallId::new(),
                name: "browser_open".into(),
                risk: open.risk(&input),
                subject: open.subject(&input),
                input,
                segments: None,
            };
            let (policy, sandbox) = (ApprovalPolicy::OnRequest, SandboxMode::WorkspaceWrite);
            engine.decide(&call, mode, policy, sandbox, &[])
        };
        let plan = decide("https://example.com", PermissionMode::Plan);
        assert!(matches!(plan, Outcome::Deny { .. }), "{plan:?}");
        let asked = decide("https://example.com", PermissionMode::Default);
        assert!(matches!(asked, Outcome::Ask(_)), "{asked:?}");
        let local = decide("http://localhost:3000", PermissionMode::Plan);
        assert!(matches!(local, Outcome::Allow { .. }), "{local:?}");
        let denied = decide("https://www.evil.test/", PermissionMode::Bypass);
        assert!(matches!(denied, Outcome::Deny { .. }), "{denied:?}");
    }

    #[test]
    fn a_typed_address_passes_the_same_scheme_rule() {
        let typed = |text: &str| web_address(text);
        assert_eq!(
            typed(" localhost:5173/checkout ").as_deref(),
            Some("http://localhost:5173/checkout")
        );
        assert_eq!(
            typed("127.0.0.1:8080").as_deref(),
            Some("http://127.0.0.1:8080/")
        );
        assert_eq!(
            typed("example.com/a").as_deref(),
            Some("https://example.com/a")
        );
        assert_eq!(
            typed("http://example.com").as_deref(),
            Some("http://example.com/")
        );
        for refused in [
            "",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "about:blank",
            "ftp://example.com",
            "data:text/html,hi",
        ] {
            assert_eq!(typed(refused), None, "{refused:?}");
        }
    }

    #[tokio::test]
    async fn browser_read_text_is_sanitized() {
        let browser = Arc::new(FakeBrowser {
            page: PageText {
                title: "Docs\x1b]0;owned\x07".into(),
                url: "https://example.com/".into(),
                text: "safe \x1b[31mred\x1b[0m \u{202e}evil".into(),
            },
            ..FakeBrowser::default()
        });
        let read = tool(&browser, "browser_read");
        let out = read.call(json!({}), &cx()).await.expect("read");
        assert!(!out.text.contains('\x1b'), "{:?}", out.text);
        assert!(!out.text.contains('\u{202e}'), "{:?}", out.text);
        assert!(out.text.starts_with("Title: Docs"), "{:?}", out.text);
        assert!(out.text.contains("URL: https://example.com/"));
        assert!(out.text.contains("red"));
    }

    #[tokio::test]
    async fn browser_screenshot_is_an_image_item() {
        let browser = Arc::new(FakeBrowser {
            shot: PNG.to_vec(),
            ..FakeBrowser::default()
        });
        let shoot = tool(&browser, "browser_screenshot");
        let mut out = shoot.call(json!({}), &cx()).await.expect("screenshot");
        let (media_type, data) = image::take_structured(&mut out).expect("an image payload");
        assert_eq!(media_type, "image/png");
        let expected = image::to_structured("image/png", PNG);
        assert_eq!(Some(data.as_str()), expected["image"]["data_b64"].as_str());

        let big = Arc::new(FakeBrowser {
            shot: [PNG, &vec![0; image::MAX_IMAGE_BYTES]].concat(),
            ..FakeBrowser::default()
        });
        let result = tool(&big, "browser_screenshot")
            .call(json!({}), &cx())
            .await;
        assert!(
            matches!(result, Err(ToolError::TooLarge { .. })),
            "{result:?}"
        );
    }
}
