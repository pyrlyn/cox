// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T3.8 step 3: `web_fetch` against a local HTTP server — readable
//! extraction, the byte cap, and the URL scheme guard — without touching
//! the network.

use std::path::PathBuf;
use std::sync::Arc;

use cox_protocol::{
    Archive, ArchiveId, ArchivePut, CallId, SandboxMode, SandboxPolicy, SessionId, StoreError,
    Tool, ToolCx, ToolError,
};
use cox_tools::web_fetch::WebFetchTool;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct NoopArchive;

#[async_trait::async_trait]
impl Archive for NoopArchive {
    async fn put(&self, _put: ArchivePut) -> Result<ArchiveId, StoreError> {
        Ok(ArchiveId::new())
    }
    async fn get(&self, _id: &ArchiveId) -> Result<Vec<u8>, StoreError> {
        Ok(Vec::new())
    }
}

fn cx() -> ToolCx {
    let (tx, _rx) = mpsc::channel(1);
    ToolCx {
        roots: vec![PathBuf::from("/tmp")],
        writable_roots: vec![PathBuf::from("/tmp")],
        cwd: PathBuf::from("/tmp"),
        sandbox: SandboxPolicy {
            mode: SandboxMode::ReadOnly,
            network: true,
            writable: vec![],
            readonly_in_workspace: vec![],
            linux_backend: Default::default(),
        },
        archive: Arc::new(NoopArchive),
        cancel: CancellationToken::new(),
        output: tx,
        session: SessionId::new(),
        call: CallId::new(),
        agent: None,
        preset: None,
        relay: None,
    }
}

/// Serves `body` with `content_type` for every request; returns the base URL.
async fn serve(content_type: &'static str, body: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let body = body.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(body.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn web_fetch_returns_readable_text_for_html() {
    let base = serve(
        "text/html; charset=utf-8",
        "<html><head><title>T</title></head><body><nav>menu</nav><main><h2>Hi</h2><p>Body \
         text.</p></main></body></html>"
            .into(),
    )
    .await;
    let out = WebFetchTool::new()
        .call(json!({"url": format!("{base}/page")}), &cx())
        .await
        .expect("fetch");
    assert!(
        out.text.starts_with("# T\n\n## Hi\n\nBody text.\n["),
        "{}",
        out.text
    );
    assert!(!out.text.contains("menu"));
    assert!(out.text.contains("/page · "), "{}", out.text);
}

#[tokio::test]
async fn web_fetch_caps_bytes_and_says_so() {
    let base = serve("text/plain", "x".repeat(50_000)).await;
    let out = WebFetchTool::new()
        .call(json!({"url": base.clone(), "max_bytes": 1000}), &cx())
        .await
        .expect("fetch");
    assert!(
        out.text.starts_with(&"x".repeat(1000)),
        "{}",
        &out.text[..40]
    );
    assert!(
        out.text.ends_with("1000 bytes; cut at max_bytes=1000]"),
        "{}",
        out.text
    );
    assert!(out.text.len() < 1200);
}

#[tokio::test]
async fn web_fetch_only_takes_http_urls_and_reports_bad_status() {
    let err = WebFetchTool::new()
        .call(json!({"url": "file:///etc/passwd"}), &cx())
        .await
        .expect_err("scheme");
    assert!(matches!(err, ToolError::Denied { .. }), "{err:?}");
    let err = WebFetchTool::new()
        .call(json!({"url": "http://127.0.0.1:9/"}), &cx())
        .await
        .expect_err("refused");
    assert!(matches!(err, ToolError::Denied { .. }), "{err:?}");
}
