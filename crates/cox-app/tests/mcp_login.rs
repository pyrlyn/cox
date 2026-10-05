// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! MCP login on the Settings screen end to end (T37.30.3): an `App` over a
//! scratch `COX_HOME` whose config names an HTTP and a stdio server. Tokens
//! live in `cox_mcp::auth::Memory` and the browser's callback is scripted,
//! so nothing opens a browser, reads the keychain (A49) or dials out. nextest
//! runs each test in its own process, so each sets its own `HOME`.

// why: env::set_var/remove_var are unsafe in edition 2024 (per-process tests).
#![allow(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cox_app::app::{App, AppError, Host};
use cox_app::mcp_login::{Flow, LoginError, McpAuth};
use cox_app::{InboxItem, McpLogin, McpStatus};
use rmcp::transport::auth::{AuthError, CredentialStore, StoredCredentials};

const LOGIN_PAGE: &str = "https://auth.example.test/authorize?client_id=cox";

/// Remembers every URL it was asked to open.
#[derive(Default)]
struct Opener(Mutex<Vec<String>>);

impl Host for Opener {
    fn notify(&self, _: InboxItem, _: u32) {}
    fn badge(&self, _: u32) {}
    fn open_url(&self, url: &str) {
        self.0.lock().expect("opened").push(url.into());
    }
    fn secret(&self, _: &str) -> Option<String> {
        None
    }
}

/// The browser, scripted: it is handed the login page, then the callback
/// brings a token that expires in an hour.
struct ScriptedCallback;

impl Flow for ScriptedCallback {
    fn login<'a>(
        &'a self,
        _url: &'a str,
        store: Arc<dyn CredentialStore>,
        open: &'a (dyn Fn(&str) + Sync),
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'a>> {
        Box::pin(async move {
            open(LOGIN_PAGE);
            let token = serde_json::from_value(serde_json::json!({
                "access_token": "tok", "token_type": "bearer", "expires_in": 3600,
            }))
            .map_err(|e| AuthError::CredentialStoreError(e.to_string()))?;
            let now = cox_mcp::auth::now();
            store
                .save(StoredCredentials::new(
                    "cid".into(),
                    Some(token),
                    vec![],
                    Some(now),
                ))
                .await
        })
    }
}

/// A home whose config has an HTTP server `docs` and a stdio server `local`,
/// and a git project to open Settings in.
fn scratch() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("user/.cox");
    let project = dir.path().join("project");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(project.join(".git")).expect(".git");
    std::fs::write(
        home.join("config.toml"),
        "[mcp.servers.docs]\nurl = \"https://mcp.example.test/mcp\"\n\n\
         [mcp.servers.local]\ncommand = \"echo\"\n",
    )
    .expect("config");
    // SAFETY: first thing in this test's own process (nextest); keeps the
    // developer's `~/.claude.json` servers out of discovery.
    unsafe { std::env::set_var("HOME", dir.path().join("user")) };
    (dir, home, project)
}

fn app(home: &Path, host: Arc<Opener>) -> Arc<App> {
    let mcp = McpAuth {
        secrets: Arc::new(cox_mcp::auth::Memory::default()),
        flow: Arc::new(ScriptedCallback),
    };
    App::with_mcp(Some(home.to_path_buf()), host, mcp).expect("app")
}

async fn logins(app: &App, project: &Path) -> Vec<(String, McpLogin)> {
    let view = app.settings(project).await.expect("settings");
    view.mcp.into_iter().map(|s| (s.name, s.login)).collect()
}

#[tokio::test]
async fn a_server_shows_logged_out_then_logged_in_after_the_callback() {
    let (_dir, home, project) = scratch();
    let host = Arc::new(Opener::default());
    let app = app(&home, Arc::clone(&host));
    assert_eq!(
        logins(&app, &project).await,
        [
            ("docs".to_string(), McpLogin::LoggedOut),
            ("local".to_string(), McpLogin::Stdio),
        ]
    );

    app.mcp_login(&project, "docs", true).await.expect("log in");
    assert_eq!(*host.0.lock().expect("opened"), [LOGIN_PAGE]);
    let logged_in = McpLogin::LoggedIn {
        expires: Some("1h".into()),
    };
    // Up to a second passes between the callback and the read.
    let now = logins(&app, &project).await.remove(0).1;
    let later = McpLogin::LoggedIn {
        expires: Some("59m".into()),
    };
    assert!(now == logged_in || now == later, "{now:?}");

    app.mcp_login(&project, "docs", false)
        .await
        .expect("log out");
    assert_eq!(logins(&app, &project).await[0].1, McpLogin::LoggedOut);
}

#[tokio::test]
async fn only_a_configured_http_server_logs_in() {
    let (_dir, home, project) = scratch();
    let host = Arc::new(Opener::default());
    let app = app(&home, Arc::clone(&host));
    let stdio = app.mcp_login(&project, "local", true).await;
    assert!(matches!(
        stdio,
        Err(AppError::McpLogin(LoginError::Stdio(_)))
    ));
    let unknown = app.mcp_login(&project, "nope", true).await;
    assert!(matches!(
        unknown,
        Err(AppError::McpLogin(LoginError::Unknown(_)))
    ));
    assert!(host.0.lock().expect("opened").is_empty());
}

/// T37.45.4: a stdio server whose program is missing is `unknown` until a
/// session in the project tries it, then `failed` with the reason in its
/// log. Unsandboxed so the spawn itself fails, with no process started.
#[tokio::test]
async fn a_server_a_session_could_not_start_shows_failed_with_its_log() {
    let (dir, home, project) = scratch();
    std::fs::write(
        home.join("config.toml"),
        "[mcp.servers.broken]\ncommand = \"/nonexistent/cox-mcp-missing\"\nsandbox = false\n",
    )
    .expect("config");
    let scenario = dir.path().join("scenario.toml");
    std::fs::write(&scenario, "[[turn]]\ntext = \"ok\"\n").expect("scenario");
    // SAFETY: this test's own process (nextest), before any session opens.
    unsafe {
        std::env::set_var("COX_PROVIDER", "scripted");
        std::env::set_var("COX_SCENARIO", &scenario);
    }
    let app = app(&home, Arc::new(Opener::default()));
    let before = app.settings(&project).await.expect("settings").mcp;
    assert_eq!(before[0].status, McpStatus::Unknown);
    assert!(before[0].log.is_empty());

    let theme = "base16-ocean.dark".to_string();
    let _session = app.open(project.clone(), None, theme).await.expect("open");
    let after = app.settings(&project).await.expect("settings").mcp;
    assert_eq!(after[0].status, McpStatus::Failed);
    assert_eq!(after[0].log.len(), 1, "{:?}", after[0].log);
    assert!(
        after[0].log[0].starts_with("skipped: spawn /nonexistent/cox-mcp-missing"),
        "{:?}",
        after[0].log
    );
}
