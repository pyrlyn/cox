// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! MCP client (plan.md T7.6): one rmcp session per server, its tools
//! exposed as `mcp__<server>__<tool>` `Tool` impls that the core gates like
//! any other. A server that will not start is a notice and no tools (D14);
//! a call that fails is an error result the model can read, never a crash.
//! HTTP servers go through rmcp's `AuthClient` (T22.5): a stored token is
//! attached and refreshed silently; a 401 on the handshake asks for a login
//! when a surface can run one, and is a notice naming `cox mcp login`
//! otherwise — never a silent skip.
//! [`CoxClient`] is the handler every session serves (T47.2): with a person
//! present it answers a server's `elicitation/create` through the surface's
//! question modal, and without one it declares nothing and declines.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use cox_protocol::config::{CHILD_ENV_ALLOWLIST, McpServerConfig};
use cox_protocol::errors::ToolError;
use cox_protocol::traits::{Tool, ToolCx};
use cox_protocol::types::{Concurrency, Risk, ToolOutput, ToolSpec};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientConfig, ElicitRequestParams, ElicitResult,
    ElicitationAction, ElicitationCapability, ErrorData, FormElicitationCapability,
    UrlElicitationCapability,
};
use rmcp::service::{ClientInitializeError, RequestContext, RoleClient, RunningService};
use rmcp::transport::auth::{AuthClient, AuthError, CredentialStore, InMemoryCredentialStore};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig, StreamableHttpError,
};
use rmcp::transport::{IntoTransport, TokioChildProcess};
use rmcp::{ClientHandler, RmcpError, ServiceExt};
use serde_json::Value;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::auth::{self, Secrets};
use crate::elicit::{self, Asker, Opener};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("server `{name}` has neither `command` nor `url`")]
    NoTransport { name: String },
    #[error("spawn {command}: {source}")]
    Spawn {
        command: String,
        source: std::io::Error,
    },
    #[error("handshake: {0}")]
    Handshake(Box<RmcpError>),
    #[error("tools/list: {0}")]
    List(rmcp::ServiceError),
    /// The server wants a token this surface cannot obtain right now.
    #[error("{} run `cox mcp login {name}`", if *.expired { "token expired," } else { "login required," })]
    LoginRequired { name: String, expired: bool },
    #[error("login: {0}")]
    Auth(#[from] AuthError),
}

/// How a surface hands the person the login URL to open.
pub type Prompt = Arc<dyn Fn(&str) + Send + Sync>;

/// What a surface brings: where tokens live and, when a person is present,
/// how to reach them — `prompt` hands them a login URL (T22.5), `ask` puts
/// a server's elicitation questions to them (T47.2). `None` for either
/// means nobody is there: a 401 is a notice, an elicitation is declined.
#[derive(Clone)]
pub struct Auth {
    pub secrets: Arc<dyn Secrets>,
    pub prompt: Option<Prompt>,
    pub ask: Option<Asker>,
}

impl Auth {
    /// No persistence, no prompt, no asker: stdio-only callers and tests.
    pub fn none() -> Self {
        Self {
            secrets: Arc::new(auth::Memory::default()),
            prompt: None,
            ask: None,
        }
    }
}

/// How many elicitations of one server wait for a person right now. A
/// call's deadline stops while any does (A78: answering must not trip
/// `mcp.timeout_s`), and a cancelled call aborts them.
#[derive(Default)]
struct Asking {
    open: AtomicUsize,
    changed: Notify,
    abort: Mutex<CancellationToken>,
}

impl Asking {
    fn busy(&self) -> bool {
        self.open.load(Ordering::SeqCst) > 0
    }

    /// Counts one open elicitation until the guard drops, however the
    /// handler's future ends.
    fn enter(self: &Arc<Self>) -> Open {
        self.open.fetch_add(1, Ordering::SeqCst);
        self.changed.notify_waiters();
        Open(self.clone())
    }

    /// The token the open elicitations watch.
    fn abort_token(&self) -> CancellationToken {
        self.abort
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Cancels every open elicitation; later ones get a fresh token.
    fn abort_all(&self) {
        let mut token = self.abort.lock().unwrap_or_else(PoisonError::into_inner);
        token.cancel();
        *token = CancellationToken::new();
    }
}

struct Open(Arc<Asking>);

impl Drop for Open {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, Ordering::SeqCst);
        self.0.changed.notify_waiters();
    }
}

/// Runs `fut` with a `timeout` that does not count while a question is
/// open; each time the last question closes the full timeout starts again.
/// `None` is a timeout.
async fn within<F: Future>(asking: &Asking, timeout: Duration, fut: F) -> Option<F::Output> {
    tokio::pin!(fut);
    loop {
        let changed = asking.changed.notified();
        tokio::pin!(changed);
        // Registered before `busy` is read, so a change in between still
        // wakes this loop.
        changed.as_mut().enable();
        if asking.busy() {
            tokio::select! {
                out = &mut fut => return Some(out),
                () = &mut changed => continue,
            }
        }
        tokio::select! {
            out = &mut fut => return Some(out),
            () = tokio::time::sleep(timeout) => return None,
            () = &mut changed => {}
        }
    }
}

/// The client side of every session (T47.2). Declares `elicitation.form`
/// and `.url` (T47.4) only when `ask` is set, and answers through it; any
/// elicitation without an asker is declined.
#[derive(Clone)]
pub struct CoxClient {
    server: String,
    ask: Option<Asker>,
    asking: Arc<Asking>,
    /// The T22.5 opener; the one way cox opens a browser.
    open: Opener,
}

impl CoxClient {
    fn new(server: &str, ask: Option<Asker>) -> Self {
        Self {
            server: server.to_string(),
            ask,
            asking: Arc::default(),
            open: Arc::new(auth::open_browser),
        }
    }
}

impl ClientHandler for CoxClient {
    fn get_info(&self) -> ClientConfig {
        let mut info = ClientConfig::default();
        if self.ask.is_some() {
            info.capabilities.elicitation = Some(
                ElicitationCapability::new()
                    .with_form(FormElicitationCapability::new().with_schema_validation(true))
                    .with_url(UrlElicitationCapability::new()),
            );
        }
        info
    }

    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        let decline = ElicitResult::new(ElicitationAction::Decline);
        let Some(asker) = &self.ask else {
            return Ok(decline);
        };
        let _open = self.asking.enter();
        let abort = self.asking.abort_token();
        let answer = async {
            match request {
                ElicitRequestParams::FormElicitationParams {
                    message,
                    requested_schema,
                    ..
                } => elicit::run_form(asker, &self.server, &message, &requested_schema).await,
                ElicitRequestParams::UrlElicitationParams { message, url, .. } => {
                    elicit::run_url(asker, &self.server, &message, &url, &self.open).await
                }
                _ => decline,
            }
        };
        Ok(tokio::select! {
            result = answer => result,
            () = context.ct.cancelled() => ElicitResult::new(ElicitationAction::Cancel),
            () = abort.cancelled() => ElicitResult::new(ElicitationAction::Cancel),
        })
    }
}

/// A connected server. Cloning shares the session; `close` ends it.
#[derive(Clone)]
pub struct McpClient {
    name: String,
    service: Arc<RunningService<RoleClient, CoxClient>>,
    timeout: Duration,
    asking: Arc<Asking>,
}

impl McpClient {
    /// Spawns a stdio server or opens a Streamable HTTP one and handshakes.
    pub async fn connect(
        name: &str,
        cfg: &McpServerConfig,
        timeout: Duration,
        auth: &Auth,
    ) -> Result<Self, ClientError> {
        match (&cfg.command, &cfg.url) {
            (Some(command), _) => {
                let mut cmd = tokio::process::Command::new(command);
                cmd.args(&cfg.args).env_clear();
                for key in CHILD_ENV_ALLOWLIST {
                    if let Ok(v) = std::env::var(key) {
                        cmd.env(key, v);
                    }
                }
                cmd.envs(&cfg.env);
                let transport =
                    TokioChildProcess::new(cmd).map_err(|source| ClientError::Spawn {
                        command: command.clone(),
                        source,
                    })?;
                Self::from_transport(name, transport, timeout, auth.ask.clone()).await
            }
            (None, Some(url)) => {
                let store = usable(auth.secrets.store(name)).await;
                let ask = auth.ask.clone();
                match Self::connect_http(name, url, store.clone(), timeout, ask.clone()).await {
                    // One login per connect, then the handshake runs again
                    // with the token the login stored.
                    Err(Login { challenge, expired }) => match &auth.prompt {
                        Some(prompt) => {
                            auth::login(url, store.clone(), Some(challenge), &**prompt).await?;
                            Self::connect_http(name, url, store, timeout, ask)
                                .await
                                .map_err(|e| e.into_client(name))
                        }
                        None => Err(ClientError::LoginRequired {
                            name: name.to_string(),
                            expired,
                        }),
                    },
                    Err(e) => Err(e.into_client(name)),
                    Ok(client) => Ok(client),
                }
            }
            (None, None) => Err(ClientError::NoTransport {
                name: name.to_string(),
            }),
        }
    }

    /// Opens a Streamable HTTP session with the stored token attached; a
    /// 401 on the handshake comes back as `Login` with the server's
    /// challenge, so the caller can decide whether anyone is there to log in.
    async fn connect_http(
        name: &str,
        url: &str,
        store: Arc<dyn CredentialStore>,
        timeout: Duration,
        ask: Option<Asker>,
    ) -> Result<Self, HttpError> {
        let (manager, expired) = auth::manager(url, store).await?;
        let client = AuthClient::new(reqwest::Client::new(), manager);
        let transport = StreamableHttpClientTransport::with_client(
            client,
            StreamableHttpClientTransportConfig::with_uri(url),
        );
        let handler = CoxClient::new(name, ask);
        let asking = handler.asking.clone();
        let service = match handler.serve(transport).await {
            Ok(service) => service,
            Err(e) => {
                return Err(match challenge_of(&e) {
                    Some(challenge) => Login { challenge, expired },
                    None => HttpError::Handshake(Box::new(e)),
                });
            }
        };
        Ok(Self {
            name: name.to_string(),
            service: Arc::new(service),
            timeout,
            asking,
        })
    }

    /// Handshakes over any rmcp transport (tests use an in-process duplex).
    /// `ask` is the surface's asker, `None` when nobody can answer.
    pub async fn from_transport<T, E, A>(
        name: &str,
        transport: T,
        timeout: Duration,
        ask: Option<Asker>,
    ) -> Result<Self, ClientError>
    where
        T: IntoTransport<RoleClient, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let handler = CoxClient::new(name, ask);
        let asking = handler.asking.clone();
        let service = handler
            .serve(transport)
            .await
            .map_err(|e| ClientError::Handshake(Box::new(e.into())))?;
        Ok(Self {
            name: name.to_string(),
            service: Arc::new(service),
            timeout,
            asking,
        })
    }

    /// Every tool the server lists, namespaced and (by default) deferred.
    pub async fn tools(&self, deferred: bool) -> Result<Vec<Arc<dyn Tool>>, ClientError> {
        let listed = self
            .service
            .list_all_tools()
            .await
            .map_err(ClientError::List)?;
        Ok(listed
            .into_iter()
            .map(|tool| {
                Arc::new(McpTool {
                    client: self.clone(),
                    tool,
                    deferred,
                }) as Arc<dyn Tool>
            })
            .collect())
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Ends the session; a stdio server's process is killed with it.
    pub async fn close(self) {
        if let Ok(service) = Arc::try_unwrap(self.service) {
            let _ = service.cancel().await;
        }
    }
}

/// A store that cannot be read (no keychain on this host, a locked secret
/// service) must not take the server down with it: the session keeps its
/// tokens in memory and `doctor` is where the keyring problem shows.
async fn usable(store: Arc<dyn CredentialStore>) -> Arc<dyn CredentialStore> {
    match store.load().await {
        Ok(_) => store,
        Err(_) => Arc::new(InMemoryCredentialStore::new()),
    }
}

use HttpError::Login;

/// Why an HTTP connect stopped; `Login` is the one the caller may recover
/// from. `expired` says credentials existed, so the notice can say "expired"
/// rather than "required".
enum HttpError {
    Login { challenge: String, expired: bool },
    Handshake(Box<ClientInitializeError>),
    Auth(AuthError),
}

impl From<AuthError> for HttpError {
    fn from(e: AuthError) -> Self {
        HttpError::Auth(e)
    }
}

impl HttpError {
    fn into_client(self, name: &str) -> ClientError {
        match self {
            Login { expired, .. } => ClientError::LoginRequired {
                name: name.to_string(),
                expired,
            },
            HttpError::Handshake(e) => ClientError::Handshake(Box::new((*e).into())),
            HttpError::Auth(e) => ClientError::Auth(e),
        }
    }
}

/// The `WWW-Authenticate` value when the handshake died on a 401/403. The
/// transport error is boxed as `dyn Error` inside rmcp's initialize error;
/// the concrete type is known because `connect_http` chose reqwest.
fn challenge_of(e: &ClientInitializeError) -> Option<String> {
    let ClientInitializeError::TransportError { error, .. } = e else {
        return None;
    };
    error
        .error
        .downcast_ref::<StreamableHttpError<reqwest::Error>>()
        .and_then(StreamableHttpError::auth_challenge)
        .map(str::to_string)
}

/// Connects every server; one that fails is a notice, not an error (step 5).
pub async fn connect_all(
    servers: &HashMap<String, McpServerConfig>,
    timeout: Duration,
    deferred: bool,
    auth: &Auth,
) -> (Vec<McpClient>, Vec<Arc<dyn Tool>>, Vec<String>) {
    let mut clients = Vec::new();
    let mut tools = Vec::new();
    let mut notices = Vec::new();
    let mut names: Vec<&String> = servers.keys().collect();
    names.sort();
    for name in names {
        // A login waits for a browser, so it gets its own budget on top of
        // the handshake timeout.
        let budget = match auth.prompt {
            Some(_) => timeout + auth::LOGIN_TIMEOUT,
            None => timeout,
        };
        let connect = tokio::time::timeout(
            budget,
            McpClient::connect(name, &servers[name], timeout, auth),
        );
        let listed = match connect.await {
            Ok(Ok(client)) => client.tools(deferred).await.map(|t| (client, t)),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                notices.push(skipped(
                    name,
                    &format!("no handshake within {}s", timeout.as_secs()),
                ));
                continue;
            }
        };
        match listed {
            Ok((client, list)) => {
                tools.extend(list);
                clients.push(client);
            }
            Err(e) => notices.push(skipped(name, &e.to_string())),
        }
    }
    (clients, tools, notices)
}

/// The notice [`connect_all`] leaves for a server it could not start;
/// [`skipped_server`] reads it back.
pub fn skipped(name: &str, reason: &str) -> String {
    format!("mcp server `{name}` skipped: {reason}")
}

/// The server and the reason in a [`skipped`] notice, so a surface can say
/// which server a session could not start (T37.45.4) without a second
/// channel beside the session's warnings. `None` for any other notice.
pub fn skipped_server(notice: &str) -> Option<(&str, &str)> {
    notice
        .strip_prefix("mcp server `")?
        .split_once("` skipped: ")
}

/// One server tool as the core sees it.
pub struct McpTool {
    client: McpClient,
    tool: rmcp::model::Tool,
    deferred: bool,
}

impl McpTool {
    fn qualified(&self) -> String {
        format!("mcp__{}__{}", self.client.name, self.tool.name)
    }
}

#[async_trait]
impl Tool for McpTool {
    fn spec(&self) -> ToolSpec {
        // Step 4: annotations are hints from an untrusted server, so only
        // `readOnlyHint` lowers the risk; `destructiveHint` raises it and
        // silence means `Write`.
        let ann = self.tool.annotations.as_ref();
        let risk = match (
            ann.and_then(|a| a.read_only_hint),
            ann.and_then(|a| a.destructive_hint),
        ) {
            (Some(true), _) => Risk::ReadOnly,
            (_, Some(true)) => Risk::Destructive,
            _ => Risk::Write,
        };
        ToolSpec {
            name: self.qualified(),
            description: self
                .tool
                .description
                .as_deref()
                .unwrap_or_default()
                .to_string(),
            input_schema: Value::Object((*self.tool.input_schema).clone()),
            deferred: self.deferred,
            risk,
            concurrency: if risk == Risk::ReadOnly {
                Concurrency::Parallel
            } else {
                Concurrency::Exclusive
            },
        }
    }

    fn subject(&self, _input: &Value) -> String {
        self.qualified()
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let mut params = CallToolRequestParams::new(self.tool.name.clone());
        if let Some(args) = input.as_object() {
            params = params.with_arguments(args.clone());
        }
        let call = self.client.service.call_tool(params);
        let asking = &self.client.asking;
        let result = tokio::select! {
            _ = cx.cancel.cancelled() => {
                // A question this server still has open would otherwise
                // wait for an answer nobody needs.
                asking.abort_all();
                return Err(ToolError::Cancelled);
            }
            r = within(asking, self.client.timeout, call) => match r {
                Some(Ok(result)) => result,
                Some(Err(e)) => return Ok(ToolOutput {
                    text: format!("mcp server `{}`: {e}", self.client.name),
                    is_error: true,
                    diff: None,
                    structured: None,
                }),
                None => return Err(ToolError::Timeout),
            },
        };
        Ok(output_of(result))
    }
}

fn output_of(result: CallToolResult) -> ToolOutput {
    let text = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    ToolOutput {
        text,
        is_error: result.is_error.unwrap_or(false),
        diff: None,
        structured: result.structured_content,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    const TOKEN: &str = "tok-1";

    /// The MCP side: answers `initialize` and `tools/list` for whatever id
    /// the client picked; notifications get the 202 the spec asks for.
    struct Rpc;

    impl Respond for Rpc {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let msg: Value = serde_json::from_slice(&req.body).unwrap_or_default();
            let result = match msg["method"].as_str() {
                Some("initialize") => json!({
                    "protocolVersion": msg["params"]["protocolVersion"],
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "t", "version": "0" },
                }),
                Some("tools/list") => json!({
                    "tools": [{ "name": "echo", "inputSchema": { "type": "object" } }]
                }),
                _ => return ResponseTemplate::new(202),
            };
            ResponseTemplate::new(200)
                .set_body_json(json!({ "jsonrpc": "2.0", "id": msg["id"], "result": result }))
        }
    }

    /// The token endpoint: a code is good, a refresh is rejected for good.
    struct TokenEndpoint;

    impl Respond for TokenEndpoint {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            if String::from_utf8_lossy(&req.body).contains("grant_type=refresh_token") {
                return ResponseTemplate::new(400)
                    .set_body_json(json!({ "error": "invalid_grant" }));
            }
            ResponseTemplate::new(200).set_body_json(json!({
                "access_token": TOKEN, "token_type": "bearer",
                "expires_in": 3600, "refresh_token": "r-1",
            }))
        }
    }

    /// One wiremock plays both the authorization server and the MCP server:
    /// no bearer → 401 with the RFC 9728 pointer; metadata, registration
    /// and token endpoints; the right bearer → the JSON-RPC answers.
    async fn oauth_server() -> MockServer {
        let server = MockServer::start().await;
        let base = server.uri();
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
            .respond_with(Rpc)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(
                ResponseTemplate::new(401).insert_header(
                    "www-authenticate",
                    format!(
                        "Bearer resource_metadata=\"{base}/.well-known/oauth-protected-resource\""
                    )
                    .as_str(),
                ),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/mcp"))
            .respond_with(ResponseTemplate::new(405))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-protected-resource"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "resource": format!("{base}/mcp"),
                "authorization_servers": [base],
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "issuer": base,
                "authorization_endpoint": format!("{base}/authorize"),
                "token_endpoint": format!("{base}/token"),
                "registration_endpoint": format!("{base}/register"),
                "response_types_supported": ["code"],
                "code_challenge_methods_supported": ["S256"],
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/register"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "client_id": "cid", "redirect_uris": [],
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(TokenEndpoint)
            .mount(&server)
            .await;
        server
    }

    fn servers(base: &str) -> HashMap<String, McpServerConfig> {
        HashMap::from([(
            "srv".to_string(),
            McpServerConfig {
                command: None,
                args: vec![],
                url: Some(format!("{base}/mcp")),
                env: HashMap::new(),
                sandbox: true,
            },
        )])
    }

    /// The test is the browser: it reads `state` and `redirect_uri` off the
    /// authorization URL and hits the loopback callback with a code.
    fn browser() -> Prompt {
        Arc::new(|url: &str| {
            let parsed = reqwest::Url::parse(url).expect("authorization url");
            let param = |k: &str| {
                parsed
                    .query_pairs()
                    .find(|(key, _)| key == k)
                    .map(|(_, v)| v.into_owned())
                    .unwrap_or_else(|| panic!("{k} in {url}"))
            };
            assert_eq!(param("code_challenge_method"), "S256");
            let callback = format!(
                "{}?code=abc&state={}",
                param("redirect_uri"),
                param("state")
            );
            tokio::spawn(async move {
                let _ = reqwest::get(callback).await;
            });
        })
    }

    #[tokio::test]
    async fn oauth_401_then_token_then_200() {
        let server = oauth_server().await;
        let secrets = Arc::new(auth::Memory::default());
        let auth = Auth {
            secrets: secrets.clone(),
            prompt: Some(browser()),
            ask: None,
        };
        let (clients, tools, notices) =
            connect_all(&servers(&server.uri()), Duration::from_secs(5), true, &auth).await;
        assert_eq!(notices, Vec::<String>::new());
        assert_eq!(clients.len(), 1);
        assert_eq!(
            tools.iter().map(|t| t.spec().name).collect::<Vec<_>>(),
            ["mcp__srv__echo"]
        );
        let stored = secrets.store("srv").load().await.expect("load");
        let token =
            serde_json::to_value(stored.expect("credentials").token_response).expect("json");
        assert_eq!(token["access_token"], TOKEN);
    }

    #[tokio::test]
    async fn oauth_refresh_failure_is_a_warning() {
        let server = oauth_server().await;
        let secrets = Arc::new(auth::Memory::default());
        let dead = serde_json::from_value(json!({
            "access_token": "old", "token_type": "bearer",
            "expires_in": 1, "refresh_token": "dead",
        }))
        .expect("token response");
        secrets
            .store("srv")
            .save(rmcp::transport::auth::StoredCredentials::new(
                "cid".into(),
                Some(dead),
                vec![],
                Some(auth::now() - 100),
            ))
            .await
            .expect("save");
        let auth = Auth {
            secrets,
            prompt: None,
            ask: None,
        };
        // No prompt means no login allowance: the handshake timeout is the
        // whole budget, and a loaded machine stalled past 5 s turned this
        // into a "no handshake" notice (T22.8). The timeout is not the claim.
        let budget = Duration::from_secs(60);
        let (clients, tools, notices) =
            connect_all(&servers(&server.uri()), budget, true, &auth).await;
        assert!(clients.is_empty() && tools.is_empty());
        assert_eq!(
            notices,
            ["mcp server `srv` skipped: token expired, run `cox mcp login srv`"]
        );
    }

    #[test]
    fn a_skipped_notice_reads_back_its_server_and_reason() {
        let notice = skipped("srv", "spawn nope: not found");
        assert_eq!(
            skipped_server(&notice),
            Some(("srv", "spawn nope: not found"))
        );
        assert_eq!(skipped_server("mcp: .mcp.json skipped: bad json"), None);
    }
}
