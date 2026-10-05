// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! OAuth for HTTP MCP servers (plan.md T22.5): where the tokens live, how a
//! login runs, and what `doctor` reports. Separate from `client.rs` because
//! the client only needs a credential store and a way to ask for a login;
//! the keyring, the loopback listener and the browser are surface details.
//!
//! The flow itself is rmcp's (`AuthorizationSession`: discovery from the 401
//! challenge, dynamic client registration, PKCE, refresh). cox adds the
//! persistent store and the redirect listener. rmcp 3.4 has no device-code
//! grant, so a headless login is the printed URL plus the loopback callback.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use rmcp::transport::auth::{
    AuthError, AuthorizationManager, AuthorizationRequest, AuthorizationSession,
    CredentialRefreshGuard, CredentialStore, InMemoryCredentialStore, StoredCredentials,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// How long a login waits for the browser to come back.
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(120);

/// Keyring service every cox credential is filed under.
const SERVICE: &str = "cox";

/// Where a server's credentials are kept. One implementation per surface:
/// the keyring for the binary, memory for tests.
pub trait Secrets: Send + Sync {
    fn store(&self, server: &str) -> Arc<dyn CredentialStore>;
}

/// The OS keyring, entry `cox/mcp/<server>`, value = rmcp's
/// `StoredCredentials` as JSON (access and refresh token, expiry, client id).
pub struct Keyring;

impl Secrets for Keyring {
    fn store(&self, server: &str) -> Arc<dyn CredentialStore> {
        Arc::new(KeyringStore {
            account: account(server),
        })
    }
}

/// In-memory stores keyed by server; what the tests and `cox mcp` (which
/// never dials out) use.
#[derive(Default)]
pub struct Memory {
    stores: Mutex<HashMap<String, Arc<InMemoryCredentialStore>>>,
}

impl Secrets for Memory {
    fn store(&self, server: &str) -> Arc<dyn CredentialStore> {
        let mut stores = self.stores.lock().unwrap_or_else(|e| e.into_inner());
        stores
            .entry(server.to_string())
            .or_insert_with(|| Arc::new(InMemoryCredentialStore::new()))
            .clone()
    }
}

fn account(server: &str) -> String {
    format!("mcp/{server}")
}

/// One keyring entry. Reads and writes go through `spawn_blocking` because
/// the platform keychain may block on a user prompt.
struct KeyringStore {
    account: String,
}

/// Reads the keyring entry for `server` synchronously (doctor has no runtime).
pub fn stored(server: &str) -> Result<Option<StoredCredentials>, AuthError> {
    read(&account(server))
}

/// `COX_KEYRING=off` (A49) makes every entry "not there": reads find nothing
/// and writes fail with a message, instead of a keychain prompt.
fn entry(account: &str) -> Result<keyring::Entry, AuthError> {
    let switch = std::env::var(cox_protocol::config::KEYRING_ENV).ok();
    if !cox_protocol::config::keyring_enabled(switch.as_deref()) {
        return Err(AuthError::CredentialStoreError(format!(
            "keyring disabled by {}",
            cox_protocol::config::KEYRING_ENV
        )));
    }
    keyring::Entry::new(SERVICE, account)
        .map_err(|e| AuthError::CredentialStoreError(e.to_string()))
}

fn read(account: &str) -> Result<Option<StoredCredentials>, AuthError> {
    let switch = std::env::var(cox_protocol::config::KEYRING_ENV).ok();
    if !cox_protocol::config::keyring_enabled(switch.as_deref()) {
        return Ok(None);
    }
    match entry(account)?.get_password() {
        Ok(json) => serde_json::from_str(&json)
            .map(Some)
            .map_err(|e| AuthError::CredentialStoreError(format!("keyring entry unreadable: {e}"))),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(AuthError::CredentialStoreError(e.to_string())),
    }
}

#[async_trait]
impl CredentialStore for KeyringStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let account = self.account.clone();
        tokio::task::spawn_blocking(move || read(&account))
            .await
            .map_err(|e| AuthError::CredentialStoreError(e.to_string()))?
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let account = self.account.clone();
        let json = serde_json::to_string(&credentials)
            .map_err(|e| AuthError::CredentialStoreError(e.to_string()))?;
        tokio::task::spawn_blocking(move || {
            entry(&account)?
                .set_password(&json)
                .map_err(|e| AuthError::CredentialStoreError(e.to_string()))
        })
        .await
        .map_err(|e| AuthError::CredentialStoreError(e.to_string()))?
    }

    async fn clear(&self) -> Result<(), AuthError> {
        let account = self.account.clone();
        tokio::task::spawn_blocking(move || match entry(&account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(AuthError::CredentialStoreError(e.to_string())),
        })
        .await
        .map_err(|e| AuthError::CredentialStoreError(e.to_string()))?
    }
}

/// rmcp takes the store by value, so a shared one is handed over through
/// this delegating wrapper.
struct Shared(Arc<dyn CredentialStore>);

#[async_trait]
impl CredentialStore for Shared {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        self.0.load().await
    }
    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        self.0.save(credentials).await
    }
    async fn clear(&self) -> Result<(), AuthError> {
        self.0.clear().await
    }
    async fn acquire_refresh_guard(&self) -> Result<Option<CredentialRefreshGuard>, AuthError> {
        self.0.acquire_refresh_guard().await
    }
}

/// A manager over `store`, primed from it: with stored credentials the
/// server's metadata is resolved and the client id configured so refresh
/// works; without, nothing is fetched. Returns whether credentials existed.
pub async fn manager(
    url: &str,
    store: Arc<dyn CredentialStore>,
) -> Result<(AuthorizationManager, bool), AuthError> {
    let mut manager = AuthorizationManager::new(url).await?;
    manager.set_credential_store(Shared(store));
    let had = manager.initialize_from_store().await?;
    Ok((manager, had))
}

/// The authorization-code flow with PKCE against `url`'s authorization
/// server: `on_url` receives the URL to open, the loopback listener on
/// `127.0.0.1:0` takes the redirect, and the token lands in `store`.
/// `challenge` is the `WWW-Authenticate` value from the server's 401, which
/// seeds discovery; without it the well-known endpoints are probed.
pub async fn login(
    url: &str,
    store: Arc<dyn CredentialStore>,
    challenge: Option<String>,
    on_url: &(dyn Fn(&str) + Sync),
) -> Result<(), AuthError> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| AuthError::AuthorizationFailed(format!("loopback listener: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| AuthError::AuthorizationFailed(format!("loopback listener: {e}")))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let mut manager = AuthorizationManager::new(url).await?;
    manager.set_credential_store(Shared(store));
    // The session registers and builds the URL; discovery is the caller's.
    let resolution = manager
        .resolve_metadata_from_challenge(challenge.as_deref())
        .await?;
    manager.set_metadata(resolution.metadata);
    let mut request = AuthorizationRequest::new(redirect_uri.clone()).with_client_name("cox");
    if let Some(challenge) = challenge {
        request = request.with_challenge(challenge);
    }
    let session = AuthorizationSession::new(manager, request)
        .await
        .map_err(|(_, e)| e)?;
    on_url(session.get_authorization_url());
    let query = tokio::time::timeout(LOGIN_TIMEOUT, callback(&listener))
        .await
        .map_err(|_| {
            AuthError::AuthorizationFailed(format!(
                "no browser callback within {}s",
                LOGIN_TIMEOUT.as_secs()
            ))
        })??;
    session
        .handle_callback_url(&format!("{redirect_uri}?{query}"))
        .await?;
    Ok(())
}

/// Accepts one HTTP request on the listener and returns its query string.
/// The reply is a plain page the person can close; nothing else is served.
async fn callback(listener: &TcpListener) -> Result<String, AuthError> {
    let (mut socket, _) = listener
        .accept()
        .await
        .map_err(|e| AuthError::AuthorizationFailed(format!("loopback accept: {e}")))?;
    let mut buf = vec![0u8; 8192];
    let n = socket
        .read(&mut buf)
        .await
        .map_err(|e| AuthError::AuthorizationFailed(format!("loopback read: {e}")))?;
    let head = String::from_utf8_lossy(&buf[..n]);
    // `GET /callback?code=..&state=.. HTTP/1.1`
    let target = head
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| AuthError::AuthorizationFailed("malformed callback request".into()))?;
    let query = target
        .split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .to_string();
    let body = "<!doctype html><title>cox</title><p>Signed in. You can close this tab.";
    let reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(reply.as_bytes()).await;
    let _ = socket.shutdown().await;
    Ok(query)
}

/// Forgets the server's credentials.
pub async fn logout(store: Arc<dyn CredentialStore>) -> Result<(), AuthError> {
    store.clear().await
}

/// Hands the URL to the platform opener; `false` when nothing could be
/// started, in which case the printed URL is all the person has.
pub fn open_browser(url: &str) -> bool {
    let os = std::env::consts::OS;
    if os != "macos"
        && os != "windows"
        && std::env::var_os("DISPLAY").is_none()
        && std::env::var_os("WAYLAND_DISPLAY").is_none()
    {
        return false;
    }
    let Some(mut cmd) = opener(os, url) else {
        return false;
    };
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The platform opener for `os` with the URL as its one last argument and no
/// shell in between: the URL comes from an MCP server, and `cmd /C start`
/// would read `& | ^ < > " %` in it as commands (T47.5). `None` unless the
/// URL is `http(s)`, since a protocol handler also runs `file:` paths and a
/// leading `-` would reach `open`/`xdg-open` as an option.
fn opener(os: &str, url: &str) -> Option<std::process::Command> {
    let lower = url.to_ascii_lowercase();
    if !lower.starts_with("https://") && !lower.starts_with("http://") {
        return None;
    }
    let mut cmd = match os {
        "macos" => std::process::Command::new("open"),
        "windows" => {
            let mut c = std::process::Command::new("rundll32");
            c.arg("url.dll,FileProtocolHandler");
            c
        }
        _ => std::process::Command::new("xdg-open"),
    };
    cmd.arg(url);
    Some(cmd)
}

/// What `doctor` prints for a server's credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// No entry: the server may simply not need a login.
    None,
    /// A token that will still be accepted, and for how long when known.
    Ok { expires_in: Option<Duration> },
    /// Expired and no refresh token to renew it: the next start warns.
    Expired,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Status::None => f.write_str("none"),
            Status::Ok { expires_in: None } => f.write_str("ok (no expiry)"),
            Status::Ok {
                expires_in: Some(d),
            } => write!(f, "ok (expires in {})", human(*d)),
            Status::Expired => f.write_str("expired"),
        }
    }
}

/// `3h`, `12m`, `40s`: how long a token has left, for `doctor` and the
/// desktop Settings screen.
pub fn human(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}h", s / 3600)
    } else if s >= 60 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

/// Classifies stored credentials at `now` (seconds since the epoch). The
/// token response is read as JSON so the crate does not depend on `oauth2`
/// for two fields.
pub fn status(creds: Option<&StoredCredentials>, now: u64) -> Status {
    let Some(creds) = creds else {
        return Status::None;
    };
    let Some(token) = creds
        .token_response
        .as_ref()
        .and_then(|t| serde_json::to_value(t).ok())
    else {
        return Status::None;
    };
    let refreshable = token
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .is_some();
    match (
        token.get("expires_in").and_then(|v| v.as_u64()),
        creds.token_received_at,
    ) {
        (Some(expires_in), Some(received)) => {
            let remaining = (received + expires_in).saturating_sub(now);
            if remaining > 0 || refreshable {
                Status::Ok {
                    expires_in: Some(Duration::from_secs(remaining)),
                }
            } else {
                Status::Expired
            }
        }
        _ => Status::Ok { expires_in: None },
    }
}

/// Seconds since the epoch, for `status`.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds(expires_in: u64, received: u64, refresh: bool) -> StoredCredentials {
        let mut token = serde_json::json!({
            "access_token": "tok", "token_type": "bearer", "expires_in": expires_in,
        });
        if refresh {
            token["refresh_token"] = "r".into();
        }
        let token = serde_json::from_value(token).expect("token response");
        StoredCredentials::new("cid".into(), Some(token), vec![], Some(received))
    }

    #[test]
    fn status_reads_expiry_and_refreshability() {
        assert_eq!(status(None, 1000), Status::None);
        assert_eq!(
            status(Some(&creds(3600, 1000, false)), 1000).to_string(),
            "ok (expires in 1h)"
        );
        assert_eq!(status(Some(&creds(10, 0, false)), 1000), Status::Expired);
        // Expired but refreshable is still fine: the next request renews it.
        assert_eq!(
            status(Some(&creds(10, 0, true)), 1000),
            Status::Ok {
                expires_in: Some(Duration::ZERO)
            }
        );
    }

    #[test]
    fn opener_passes_the_url_as_one_argument_without_a_shell() {
        use std::ffi::OsStr;
        // `cmd /C start` would run `calc` here; every opener must see one URL.
        let url = "https://example.com/cb?a=1&b=2|calc^x<y>\"%PATH%";
        let cases: [(&str, &[&str]); 3] = [
            ("windows", &["rundll32", "url.dll,FileProtocolHandler", url]),
            ("macos", &["open", url]),
            ("linux", &["xdg-open", url]),
        ];
        for (os, argv) in cases {
            let cmd = opener(os, url).expect("http(s) URL");
            let got: Vec<&OsStr> = std::iter::once(cmd.get_program())
                .chain(cmd.get_args())
                .collect();
            let want: Vec<&OsStr> = argv.iter().map(OsStr::new).collect();
            assert_eq!(got, want, "{os}");
        }
        for url in [
            "file:///C:/Windows/System32/calc.exe",
            "javascript:alert(1)",
            "-a Calculator",
        ] {
            assert!(opener("windows", url).is_none(), "{url}");
            assert!(opener("macos", url).is_none(), "{url}");
        }
    }

    #[tokio::test]
    async fn memory_secrets_hand_out_one_store_per_server() {
        let secrets = Memory::default();
        secrets
            .store("a")
            .save(creds(1, 1, false))
            .await
            .expect("save");
        assert!(secrets.store("a").load().await.expect("load").is_some());
        assert!(secrets.store("b").load().await.expect("load").is_none());
    }
}
