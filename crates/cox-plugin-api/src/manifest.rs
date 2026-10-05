//! The `plugin.toml` manifest (PL§2): the typed tree a package declares and
//! the structural checks every consumer must agree on. Kept apart from the
//! host so the guest SDK, `cox plugin new` and the host all validate against
//! one definition; reading the file and the checks that need the filesystem
//! (the `id` matches its directory) stay with the loader in `cox-plugin`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The ABI major this crate describes. The host refuses any other `api`.
pub const API_MAJOR: u32 = 1;

/// A whole `plugin.toml`. Unknown keys are an error at every level: the
/// manifest is ours, and a misspelt capability must not silently grant less
/// (or be read later as more) than the author meant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    /// ABI major the package was built against.
    pub api: u32,
    /// Package id, `^[a-z][a-z0-9-]{1,23}$`.
    pub id: String,
    /// Package version as the author writes it.
    pub version: String,
    /// Human-readable name shown at approval.
    pub name: String,
    /// One-line description shown at approval.
    #[serde(default)]
    pub description: String,
    /// The module to load, relative to the package directory. Absent only
    /// for a data-only package — `[[mcp]]` servers and/or `[[agents]]`
    /// files (PL§13/§14, T33.38, T45.3): `validate` then requires
    /// `capabilities` to be its default and `provider`/`models`/
    /// `external_agents` to be empty, since every other capability class
    /// needs a wasm export to back it.
    pub wasm: Option<String>,
    /// True for wasip1 guests; no preopens unless `capabilities.fs` grants them.
    #[serde(default)]
    pub wasi: bool,
    /// Resource requests; the host clamps them to its maxima.
    #[serde(default)]
    pub limits: Limits,
    /// Everything the plugin asks for; each entry is one approval line.
    #[serde(default)]
    pub capabilities: Capabilities,
    /// `[[provider]]` sections (PL§7a).
    #[serde(default)]
    pub provider: Vec<ProviderDecl>,
    /// `[[models]]` catalog rows (PL§7b).
    #[serde(default)]
    pub models: Vec<ModelDecl>,
    /// `[[mcp]]` server declarations (PL§7c).
    #[serde(default)]
    pub mcp: Vec<McpDecl>,
    /// `[[external_agents]]` entries: a plugin-brought external agent CLI
    /// driven over ACP or `stream-json` (P35, EA§1 in
    /// `docs/design/external-agents.md`).
    #[serde(default)]
    pub external_agents: Vec<ExternalAgentDecl>,
    /// `[[cloud_agents]]` entries: a background task run as a hosted cloud
    /// agent (P56, `docs/design/v0.3-cursor-cloud.md`). The host drives it;
    /// no wasm export backs it.
    #[serde(default)]
    pub cloud_agents: Vec<CloudAgentDecl>,
    /// `[[agents]]` subagent definition files the package ships (T45.3);
    /// each is one approval line, and a local definition of the same name
    /// wins over it.
    #[serde(default)]
    pub agents: Vec<AgentDecl>,
}

/// `[limits]`. Absent means "the host default".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Linear memory in MiB.
    pub memory_mib: Option<u32>,
    /// Default per-call budget in milliseconds.
    pub call_ms: Option<u32>,
}

/// `[capabilities]`: the unit of approval.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    /// `Event` serde tags the plugin subscribes to.
    #[serde(default)]
    pub events: Vec<String>,
    /// `HookEvent` names the plugin handles.
    #[serde(default)]
    pub hooks: Vec<String>,
    /// Tools it exports; each becomes `wasm__<id>__<tool>`.
    #[serde(default)]
    pub tools: Vec<String>,
    /// Tools it may call; each call still passes the permission engine.
    #[serde(default)]
    pub invoke: Vec<String>,
    /// The event-folded context snapshot.
    #[serde(default)]
    pub context: bool,
    /// The per-plugin key-value store.
    #[serde(default)]
    pub kv: bool,
    /// Tier ceiling for `cox_model_call`; absent means no model access.
    pub model: Option<ModelTier>,
    /// Host patterns `cox_http` may reach.
    #[serde(default)]
    pub net: Vec<String>,
    /// WASI preopens.
    #[serde(default)]
    pub fs: FsCaps,
    /// Decision points (PL§6b) the plugin can answer.
    #[serde(default)]
    pub decide: Vec<String>,
    /// TUI contributions (PL§8).
    #[serde(default)]
    pub ui: UiCaps,
}

/// The highest tier a plugin may ask the router for. `think` is not a
/// variant, so no manifest can request it (D5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ModelTier {
    /// The low-cost tier.
    Cheap,
    /// The coding tier; must be named explicitly.
    Code,
}

impl Capabilities {
    /// Whether `host` is in `net`: an exact name, or a strict subdomain of
    /// a `*.` pattern (`*.github.com` does not cover `github.com`). The one
    /// matcher for every `net` check — an `[[mcp]]` url (PL§7c) and `cox_http`.
    pub fn net_allows(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        self.net.iter().any(|pattern| {
            let pattern = pattern.to_ascii_lowercase();
            match pattern.strip_prefix("*.") {
                Some(base) => host
                    .strip_suffix(base)
                    .is_some_and(|sub| sub.len() > 1 && sub.ends_with('.')),
                None => host == pattern,
            }
        })
    }
}

/// `capabilities.fs`: roots are `$WORKSPACE`, `$PLUGIN_DATA` or inside them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsCaps {
    /// Read-only preopens.
    #[serde(default)]
    pub read: Vec<String>,
    /// Read-write preopens.
    #[serde(default)]
    pub write: Vec<String>,
}

/// `capabilities.ui`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiCaps {
    /// Status-line segments.
    #[serde(default)]
    pub status: bool,
    /// A bottom panel.
    #[serde(default)]
    pub panel: bool,
    /// An overlay.
    #[serde(default)]
    pub overlay: bool,
    /// Slash commands.
    #[serde(default)]
    pub commands: bool,
    /// Keys under the plugin leader.
    #[serde(default)]
    pub keys: bool,
    /// Render targets: `tool:<name>` or `item:assistant_message`.
    #[serde(default)]
    pub render: Vec<String>,
}

/// A `[[provider]]` section (PL§7a).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderDecl {
    /// Section name, as a tier's `provider` refers to it.
    pub name: String,
    /// Which wire drives it.
    pub api: ProviderApi,
    /// Endpoint; for `plugin`, the only host its `cox_http` may reach.
    pub base_url: String,
    /// Env var the host resolves the key from; the key never enters wasm.
    pub api_key_env: Option<String>,
    /// How the host attaches the key.
    #[serde(default)]
    pub auth: ProviderAuth,
}

/// The wire a provider section speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ProviderApi {
    /// OpenAI Chat Completions, driven by cox's own client.
    Chat,
    /// OpenAI Responses, driven by cox's own client.
    Responses,
    /// The plugin's `cox_provider_stream` export.
    Plugin,
}

/// The auth header the host adds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderAuth {
    /// `Authorization: Bearer <key>`.
    #[default]
    Bearer,
    /// `x-api-key: <key>`.
    XApiKey,
    /// No key.
    None,
}

/// A `[[models]]` row: a fill-only catalog layer (PL§7b).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelDecl {
    /// Model id as sent on the wire.
    pub id: String,
    /// The provider section serving it.
    pub provider: Option<String>,
    /// Context window in tokens.
    pub context_window: Option<u32>,
    /// Max output tokens.
    pub max_output: Option<u32>,
    /// USD per million tokens.
    pub price: Option<PriceDecl>,
}

/// A `[[models]]` price, USD per million tokens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PriceDecl {
    /// Input tokens.
    pub input: f64,
    /// Output tokens.
    pub output: f64,
    /// Cache-read tokens.
    pub cache_read: Option<f64>,
    /// Cache-write tokens.
    pub cache_write: Option<f64>,
}

/// A `[[mcp]]` server (PL§7c): a stdio `command` or an HTTP `url`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpDecl {
    /// Server name; tools appear as `mcp__<id>-<name>__<tool>`.
    pub name: String,
    /// Program inside the package or on PATH.
    pub command: Option<String>,
    /// Arguments for `command`.
    #[serde(default)]
    pub args: Vec<String>,
    /// Streamable-HTTP endpoint.
    pub url: Option<String>,
}

/// A `[[external_agents]]` entry (EA§1): a host-spawned external agent CLI,
/// never a WASM export — a guest cannot open a process (EA§2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExternalAgentDecl {
    /// The `agent(preset: "<name>")` dispatch name (EA§1, EA§3).
    pub name: String,
    /// Program inside the package or on PATH.
    pub command: String,
    /// Arguments for `command`.
    #[serde(default)]
    pub args: Vec<String>,
    /// Which headless protocol the process speaks.
    pub mode: AgentMode,
    /// Env var the host resolves the key from with `resolve_key` (D12/A49);
    /// a name, never a value — the key never enters wasm and is never
    /// hand-typed into the manifest.
    pub key_env: String,
}

/// A `[[cloud_agents]]` entry (P56): a background task run as a hosted
/// cloud agent. There is deliberately no URL field: the host for a backend
/// is fixed in cox, so a plugin cannot point the user's key at another
/// server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CloudAgentDecl {
    /// The `agent(preset: "<name>")` dispatch name.
    pub name: String,
    /// Which hosted service runs it.
    pub backend: CloudBackend,
    /// Env var the host resolves the key from; a name, never a value.
    #[serde(default = "default_cloud_key_env")]
    pub key_env: String,
    /// The backend's model id; the backend's own default when absent.
    #[serde(default)]
    pub model: Option<String>,
    /// One-line description shown at approval.
    #[serde(default)]
    pub description: Option<String>,
}

fn default_cloud_key_env() -> String {
    String::from("CURSOR_API_KEY")
}

/// The hosted services a `[[cloud_agents]]` entry can name. Closed: each
/// variant fixes a host (`CloudBackend::host`), so adding one is a reviewed
/// code change rather than a manifest string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CloudBackend {
    /// Cursor Cloud Agents (`api.cursor.com`).
    Cursor,
}

impl CloudBackend {
    /// The name the manifest and the approval line use.
    pub fn name(self) -> &'static str {
        match self {
            Self::Cursor => "cursor",
        }
    }

    /// The one host this backend's requests go to.
    pub fn host(self) -> &'static str {
        match self {
            Self::Cursor => "api.cursor.com",
        }
    }
}

/// An `[[agents]]` entry (T45.3): a subagent definition (`.md` with
/// frontmatter, the same format as `.cox/agents/*.md`) inside the package.
/// Data, not code: the host reads it only for a granted plugin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentDecl {
    /// The `agent(preset: "<name>")` dispatch name.
    pub name: String,
    /// The definition file, relative to the package directory, ending in
    /// `.md`; no `..` component.
    pub file: String,
}

/// The headless protocol an `[[external_agents]]` entry speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AgentMode {
    /// Agent Client Protocol over stdio (EA§4): cox is the client.
    Acp,
    /// The line-oriented `stream-json` protocol (EA§5): a pure host-side
    /// line mapper onto `cox_protocol::Event`/`Item`.
    StreamJson,
}

/// Why a parsed manifest is refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ManifestError {
    /// `api` is not [`API_MAJOR`].
    #[error("api = {0}, but this host speaks api = {API_MAJOR}")]
    ApiMajor(u32),
    /// `id` breaks `^[a-z][a-z0-9-]{1,23}$`.
    #[error("plugin id {0:?} must match ^[a-z][a-z0-9-]{{1,23}}$")]
    Id(String),
    /// A name would break the model tool-name rule once prefixed.
    #[error("{0:?} does not fit [a-zA-Z0-9_-]{{1,64}} once prefixed")]
    ToolName(String),
    /// A `net` entry is a URL or otherwise not a host pattern.
    #[error("net entry {0:?} must be a host pattern such as api.github.com, not a URL")]
    NetHost(String),
    /// An `fs` root escapes `$WORKSPACE`/`$PLUGIN_DATA`.
    #[error("fs root {0:?} must be $WORKSPACE, $PLUGIN_DATA or a path inside them")]
    FsRoot(String),
    /// A `ui.render` target is neither `tool:<name>` nor `item:assistant_message`.
    #[error("ui.render target {0:?} must be tool:<name> or item:assistant_message")]
    RenderTarget(String),
    /// An `[[mcp]]` entry has both or neither of `command` and `url`.
    #[error("[[mcp]] {0:?} needs exactly one of command or url")]
    McpTransport(String),
    /// A `key_env` is not an env var name.
    #[error("key_env {0:?} must be an env var name such as CURSOR_API_KEY, not a value")]
    KeyEnv(String),
    /// `wasm` is absent, but the manifest declares something besides
    /// `[[mcp]]` servers and `[[agents]]` files: only a data-only package
    /// may omit `wasm`.
    #[error("wasm is required unless the package only declares [[mcp]] and [[agents]] (PL§13/§14)")]
    MissingWasm,
    /// An `[[agents]]` file is absolute, climbs out with `..`, or is not a
    /// `.md` file.
    #[error("[[agents]] file {0:?} must be a relative .md path inside the package, with no ..")]
    AgentFile(String),
    /// Two `[[agents]]` entries share a name.
    #[error("[[agents]] name {0:?} is declared twice")]
    DuplicateAgent(String),
}

impl PluginManifest {
    /// The structural checks of PL§2 that need nothing but the manifest.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.api != API_MAJOR {
            return Err(ManifestError::ApiMajor(self.api));
        }
        if !is_plugin_id(&self.id) {
            return Err(ManifestError::Id(self.id.clone()));
        }
        if self.wasm.is_none() {
            // `[[mcp]]` and `[[agents]]` need no wasm export to back them.
            let data_only = self.capabilities == Capabilities::default()
                && self.provider.is_empty()
                && self.models.is_empty()
                && self.external_agents.is_empty()
                && self.cloud_agents.is_empty();
            if !data_only {
                return Err(ManifestError::MissingWasm);
            }
        }
        let caps = &self.capabilities;
        for tool in &caps.tools {
            if !is_tool_name(&format!("wasm__{}__{tool}", self.id)) {
                return Err(ManifestError::ToolName(tool.clone()));
            }
        }
        if let Some(bad) = caps.invoke.iter().find(|t| !is_tool_name(t)) {
            return Err(ManifestError::ToolName(bad.clone()));
        }
        if let Some(bad) = caps.net.iter().find(|h| !is_host_pattern(h)) {
            return Err(ManifestError::NetHost(bad.clone()));
        }
        if let Some(bad) = caps
            .fs
            .read
            .iter()
            .chain(&caps.fs.write)
            .find(|r| !is_fs_root(r))
        {
            return Err(ManifestError::FsRoot(bad.clone()));
        }
        for target in &caps.ui.render {
            let ok = target == "item:assistant_message"
                || target.strip_prefix("tool:").is_some_and(is_tool_name);
            if !ok {
                return Err(ManifestError::RenderTarget(target.clone()));
            }
        }
        for server in &self.mcp {
            // cox-mcp splits `mcp__<server>__<tool>` at the first `__`, so the
            // server part must not contain one; a one-char tool must still fit.
            let prefixed = format!("mcp__{}-{}__x", self.id, server.name);
            if server.name.contains("__") || !is_tool_name(&prefixed) {
                return Err(ManifestError::ToolName(server.name.clone()));
            }
            if server.command.is_some() == server.url.is_some() {
                return Err(ManifestError::McpTransport(server.name.clone()));
            }
        }
        for agent in &self.external_agents {
            // Same "fits after prefixing" rule as an `[[mcp]]` server name
            // (EA§1): the id-joined form must still be a valid tool name.
            let prefixed = format!("{}-{}", self.id, agent.name);
            if !is_tool_name(&prefixed) {
                return Err(ManifestError::ToolName(agent.name.clone()));
            }
            if !is_env_var_name(&agent.key_env) {
                return Err(ManifestError::KeyEnv(agent.key_env.clone()));
            }
        }
        for agent in &self.cloud_agents {
            if !is_tool_name(&format!("{}-{}", self.id, agent.name)) {
                return Err(ManifestError::ToolName(agent.name.clone()));
            }
            if !is_env_var_name(&agent.key_env) {
                return Err(ManifestError::KeyEnv(agent.key_env.clone()));
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for agent in &self.agents {
            if !is_tool_name(&format!("{}-{}", self.id, agent.name)) {
                return Err(ManifestError::ToolName(agent.name.clone()));
            }
            if !is_package_md(&agent.file) {
                return Err(ManifestError::AgentFile(agent.file.clone()));
            }
            if !seen.insert(agent.name.as_str()) {
                return Err(ManifestError::DuplicateAgent(agent.name.clone()));
            }
        }
        Ok(())
    }
}

/// A `.md` path inside the package: `/`-separated, relative, no empty,
/// `..`, `\` or `:` (a Windows drive) component. The loader still confines
/// the joined path under the package directory (T45.4); this refuses an
/// escape before approval shows it.
fn is_package_md(s: &str) -> bool {
    s.ends_with(".md")
        && !s.starts_with('/')
        && s.split('/')
            .all(|c| !c.is_empty() && c != ".." && !c.contains(['\\', ':']))
}

/// `^[a-z][a-z0-9-]{1,23}$`. The charset has no `_`, which is what keeps the
/// `__` separator in `wasm__<id>__<tool>` unambiguous (PL§1). `pub` so
/// `cox plugin new` (PL§13) validates a candidate name with this exact rule
/// instead of a second regex.
pub fn is_plugin_id(s: &str) -> bool {
    (2..=24).contains(&s.len())
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The model tool-name rule every provider accepts: `[a-zA-Z0-9_-]{1,64}`.
fn is_tool_name(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A shell environment-variable name: `[A-Za-z_][A-Za-z0-9_]*`. `key_env`
/// names the var the host resolves at spawn time (`resolve_key`); it must
/// never look like a value (a key, a URL, a path), which this shape rules out.
fn is_env_var_name(s: &str) -> bool {
    let mut bytes = s.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// A DNS name, optionally `*.`-prefixed. Rejecting `:`, `/` and `@` is what
/// keeps a scheme, port, path or userinfo out: the allow-list matches hosts,
/// and a URL here would read as a narrower grant than the host check enforces.
fn is_host_pattern(s: &str) -> bool {
    let host = s.strip_prefix("*.").unwrap_or(s);
    (1..=253).contains(&host.len())
        && host.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// `$WORKSPACE` or `$PLUGIN_DATA`, optionally followed by `/`-separated
/// components with no `.`, `..` or `\`. `path::confine` still checks the
/// resolved path at grant time; this refuses an escape before approval shows it.
fn is_fs_root(s: &str) -> bool {
    let rest = ["$WORKSPACE", "$PLUGIN_DATA"]
        .iter()
        .find_map(|base| s.strip_prefix(base));
    match rest {
        Some("") => true,
        Some(rest) => rest.strip_prefix('/').is_some_and(|path| {
            path.split('/')
                .all(|c| !c.is_empty() && c != "." && c != ".." && !c.contains('\\'))
        }),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use figment::Figment;
    use figment::providers::{Format, Toml};

    use super::*;

    const EXAMPLE: &str = r#"
api = 1
id = "git-glance"
version = "0.3.0"
name = "Git glance"
description = "Branch and CI state in the status line"
wasm = "plugin.wasm"

[limits]
memory_mib = 16
call_ms = 200

[capabilities]
events = ["turn_started", "tool_call_done", "usage"]
hooks = ["PreToolUse", "PostToolUse"]
tools = ["summarise"]
invoke = ["read", "grep"]
context = true
kv = true
model = "cheap"
net = ["api.github.com"]
fs = { read = ["$WORKSPACE"], write = [] }
decide = ["risk"]
ui = { status = true, panel = true, overlay = false, commands = true, keys = true, render = ["tool:wasm__git-glance__summarise"] }

[[provider]]
name = "typesafe"
api = "plugin"
base_url = "https://api.typesafe.ai"
api_key_env = "TYPESAFE_API_KEY"
auth = "bearer"

[[models]]
id = "jev-latest"
provider = "typesafe"
context_window = 64000
price = { input = 0.042, output = 0.0 }

[[mcp]]
name = "gh"
command = "bin/gh-mcp"
args = ["--stdio"]
"#;

    /// figment's error is large, so tests carry only its message.
    fn parse(toml: &str) -> Result<PluginManifest, String> {
        Figment::from(Toml::string(toml))
            .extract()
            .map_err(|e| e.to_string())
    }

    fn example() -> PluginManifest {
        parse(EXAMPLE).expect("the PL§2 example parses")
    }

    #[test]
    fn design_example_parses_and_validates() {
        let m = example();
        assert_eq!(m.capabilities.model, Some(ModelTier::Cheap));
        assert_eq!(m.provider[0].api, ProviderApi::Plugin);
        assert_eq!(m.validate(), Ok(()));
    }

    #[test]
    fn manifest_rejects_unknown_keys() {
        let top = EXAMPLE.replace("wasm = ", "wasm_path = \"x\"\nwasm = ");
        let nested = EXAMPLE.replace("kv = true", "kv = true\nshell = true");
        for toml in [top, nested] {
            let err = parse(&toml).expect_err("an unknown key is refused");
            assert!(err.contains("unknown field"), "{err}");
        }
    }

    #[test]
    fn manifest_rejects_think_tier() {
        assert!(parse(&EXAMPLE.replace("\"cheap\"", "\"think\"")).is_err());
    }

    #[test]
    fn manifest_rejects_net_url() {
        for entry in [
            "https://api.github.com",
            "api.github.com/repos",
            "api.github.com:443",
            "user@api.github.com",
            "",
        ] {
            let mut m = example();
            m.capabilities.net = vec![entry.into()];
            assert_eq!(m.validate(), Err(ManifestError::NetHost(entry.into())));
        }
        let mut m = example();
        m.capabilities.net = vec!["*.githubusercontent.com".into()];
        assert_eq!(m.validate(), Ok(()));
    }

    #[test]
    fn net_allows_exact_hosts_and_strict_subdomains_of_a_wildcard() {
        let mut m = example();
        m.capabilities.net = vec!["api.github.com".into(), "*.example.com".into()];
        let caps = &m.capabilities;
        assert!(caps.net_allows("API.github.com"));
        assert!(caps.net_allows("a.b.example.com"));
        assert!(!caps.net_allows("example.com"));
        assert!(!caps.net_allows("evilexample.com"));
        assert!(!caps.net_allows("github.com"));
    }

    #[test]
    fn manifest_rejects_id_with_double_underscore() {
        for id in [
            "git__glance",
            "Git-glance",
            "g",
            "1git",
            "a-very-long-plugin-id-xyz",
        ] {
            let mut m = example();
            m.id = id.into();
            assert_eq!(m.validate(), Err(ManifestError::Id(id.into())));
        }
    }

    #[test]
    fn manifest_rejects_tool_name_too_long_after_prefix() {
        let mut m = example();
        // 64 on its own, but not after `wasm__git-glance__`.
        m.capabilities.tools = vec!["t".repeat(64)];
        assert_eq!(m.validate(), Err(ManifestError::ToolName("t".repeat(64))));
    }

    #[test]
    fn manifest_rejects_fs_root_outside_workspace_or_data() {
        for root in [
            "/etc",
            "$HOME",
            "$WORKSPACE/../x",
            "$WORKSPACEX",
            "$PLUGIN_DATA/",
        ] {
            let mut m = example();
            m.capabilities.fs.read = vec![root.into()];
            assert_eq!(m.validate(), Err(ManifestError::FsRoot(root.into())));
        }
        let mut m = example();
        m.capabilities.fs.write = vec!["$PLUGIN_DATA/cache".into()];
        assert_eq!(m.validate(), Ok(()));
    }

    #[test]
    fn manifest_rejects_bare_render_target() {
        let mut m = example();
        m.capabilities.ui.render = vec!["read".into()];
        assert_eq!(
            m.validate(),
            Err(ManifestError::RenderTarget("read".into()))
        );
    }

    #[test]
    fn manifest_rejects_mcp_with_both_command_and_url() {
        let mut m = example();
        m.mcp[0].url = Some("https://example.com/mcp".into());
        assert_eq!(m.validate(), Err(ManifestError::McpTransport("gh".into())));
    }

    #[test]
    fn manifest_rejects_other_api_major() {
        let mut m = example();
        m.api = 2;
        assert_eq!(m.validate(), Err(ManifestError::ApiMajor(2)));
    }

    /// PL§13/§14 (T33.38): a package whose only capability is an `[[mcp]]`
    /// server may ship no `plugin.wasm` at all.
    #[test]
    fn manifest_allows_a_wasm_less_mcp_only_package() {
        let toml = r#"
api = 1
id = "count-dart"
version = "0.1.0"
name = "count"

[[mcp]]
name = "count"
command = "build/count"
"#;
        let m = parse(toml).expect("a wasm-less mcp-only manifest parses");
        assert_eq!(m.wasm, None);
        assert_eq!(m.validate(), Ok(()));
    }

    /// The same package, but it also declares a wasm-backed capability
    /// (PL§13/§14): nothing but `[[mcp]]` may go without `wasm`.
    #[test]
    fn manifest_rejects_a_wasm_less_package_that_declares_a_capability() {
        let toml = r#"
api = 1
id = "count-dart"
version = "0.1.0"
name = "count"

[capabilities]
tools = ["summarise"]

[[mcp]]
name = "count"
command = "build/count"
"#;
        let m = parse(toml).expect("still a structurally valid manifest");
        assert_eq!(m.validate(), Err(ManifestError::MissingWasm));
    }

    /// Every non-`[[mcp]]` list that also gates the wasm-less exemption:
    /// `provider`, `models`, `external_agents` and `cloud_agents` each need `wasm` too.
    #[test]
    fn manifest_rejects_a_wasm_less_package_with_provider_models_or_agents() {
        let base = r#"
api = 1
id = "count-dart"
version = "0.1.0"
name = "count"
"#;
        let provider = format!(
            "{base}\n[[provider]]\nname = \"p\"\napi = \"chat\"\nbase_url = \"https://x\"\n"
        );
        let models = format!("{base}\n[[models]]\nid = \"m\"\n");
        let cloud = format!("{base}\n[[cloud_agents]]\nname = \"c\"\nbackend = \"cursor\"\n");
        let agents = format!(
            "{base}\n[[external_agents]]\nname = \"a\"\ncommand = \"agent\"\nmode = \"acp\"\nkey_env = \"A_KEY\"\n"
        );
        for toml in [provider, models, agents, cloud] {
            let m = parse(&toml).expect("structurally valid manifest");
            assert_eq!(m.validate(), Err(ManifestError::MissingWasm));
        }
    }

    #[test]
    fn manifest_cloud_agent_defaults_the_key_env_and_round_trips() {
        let toml = format!(
            "{EXAMPLE}\n[[cloud_agents]]\nname = \"bg\"\nbackend = \"cursor\"\nmodel = \"composer-2\"\n"
        );
        let m = parse(&toml).expect("parses");
        assert_eq!(m.validate(), Ok(()));
        assert_eq!(
            m.cloud_agents,
            vec![CloudAgentDecl {
                name: "bg".into(),
                backend: CloudBackend::Cursor,
                key_env: "CURSOR_API_KEY".into(),
                model: Some("composer-2".into()),
                description: None,
            }]
        );
        assert_eq!(CloudBackend::Cursor.host(), "api.cursor.com");
    }

    #[test]
    fn manifest_cloud_agent_backend_is_a_closed_set() {
        let toml = format!("{EXAMPLE}\n[[cloud_agents]]\nname = \"bg\"\nbackend = \"devin\"\n");
        assert!(parse(&toml).is_err());
    }

    #[test]
    fn manifest_cloud_agent_has_no_url_field() {
        for field in ["url", "base_url", "host"] {
            let toml = format!(
                "{EXAMPLE}\n[[cloud_agents]]\nname = \"bg\"\nbackend = \"cursor\"\n{field} = \"https://evil.example\"\n"
            );
            assert!(parse(&toml).is_err(), "{field} must be refused");
        }
    }

    #[test]
    fn manifest_cloud_agent_rejects_a_key_value_in_key_env() {
        let toml = format!(
            "{EXAMPLE}\n[[cloud_agents]]\nname = \"bg\"\nbackend = \"cursor\"\nkey_env = \"sk-live-123\"\n"
        );
        let m = parse(&toml).expect("a bad key_env is still a string, so this parses");
        assert_eq!(
            m.validate(),
            Err(ManifestError::KeyEnv("sk-live-123".into()))
        );
    }

    /// T45.3: an `[[agents]]` entry, appended to the PL§2 fixture.
    const AGENT: &str = r#"
[[agents]]
name = "reviewer"
file = "agents/reviewer.md"
"#;

    #[test]
    fn manifest_agents_reject_parent_dir_file() {
        let m = parse(&format!("{EXAMPLE}\n{AGENT}")).expect("parses");
        assert_eq!(
            m.agents,
            vec![AgentDecl {
                name: "reviewer".into(),
                file: "agents/reviewer.md".into(),
            }]
        );
        assert_eq!(m.validate(), Ok(()));
        for bad in [
            "../reviewer.md",
            "agents/../../x.md",
            "/etc/agent.md",
            "agents\\..\\x.md",
            "C:/x.md",
            "agents//x.md",
            "agents/reviewer.txt",
        ] {
            let mut m = m.clone();
            m.agents[0].file = bad.into();
            assert_eq!(m.validate(), Err(ManifestError::AgentFile(bad.into())));
        }
        let mut twice = m.clone();
        twice.agents.push(twice.agents[0].clone());
        assert_eq!(
            twice.validate(),
            Err(ManifestError::DuplicateAgent("reviewer".into()))
        );
        let mut long = m;
        long.agents[0].name = "r".repeat(64);
        assert_eq!(
            long.validate(),
            Err(ManifestError::ToolName("r".repeat(64)))
        );
    }

    /// T45.3: agent files are data, so a package of only `[[agents]]` (and
    /// `[[mcp]]`) needs no `plugin.wasm`.
    #[test]
    fn manifest_agents_only_package_needs_no_wasm() {
        let toml = format!(
            "api = 1\nid = \"review-kit\"\nversion = \"0.1.0\"\nname = \"review\"\n{AGENT}"
        );
        let m = parse(&toml).expect("a wasm-less agents-only manifest parses");
        assert_eq!(m.wasm, None);
        assert_eq!(m.validate(), Ok(()));
        let with_tool = format!("{toml}\n[capabilities]\ntools = [\"summarise\"]\n");
        let m = parse(&with_tool).expect("structurally valid");
        assert_eq!(m.validate(), Err(ManifestError::MissingWasm));
    }

    /// EA§1's `plugin.toml` example, appended to the PL§2 fixture.
    const EXTERNAL_AGENT: &str = r#"
[[external_agents]]
name = "cursor"
command = "agent"
mode = "acp"
key_env = "CURSOR_API_KEY"
"#;

    #[test]
    fn external_agent_entry_round_trips_through_toml() {
        let toml = format!("{EXAMPLE}\n{EXTERNAL_AGENT}");
        let m = parse(&toml).expect("EA§1's example parses");
        assert_eq!(
            m.external_agents,
            vec![ExternalAgentDecl {
                name: "cursor".into(),
                command: "agent".into(),
                args: vec![],
                mode: AgentMode::Acp,
                key_env: "CURSOR_API_KEY".into(),
            }]
        );
        assert_eq!(m.validate(), Ok(()));
    }

    #[test]
    fn unknown_mode_value_is_a_validation_error() {
        let toml = format!(
            "{EXAMPLE}\n{}",
            EXTERNAL_AGENT.replace("\"acp\"", "\"yolo\"")
        );
        assert!(parse(&toml).is_err(), "an unknown mode must be refused");
    }

    #[test]
    fn manifest_rejects_external_agent_key_env_that_is_not_an_identifier() {
        for bad in ["CURSOR-API-KEY", "1CURSOR", "sk-abc123", "", "CURSOR KEY"] {
            let toml = format!(
                "{EXAMPLE}\n{}",
                EXTERNAL_AGENT.replace("CURSOR_API_KEY", bad)
            );
            let m = parse(&toml).expect("a bad key_env is still a string, so this parses");
            assert_eq!(m.validate(), Err(ManifestError::KeyEnv(bad.into())));
        }
    }

    #[test]
    fn manifest_rejects_external_agent_name_too_long_after_prefix() {
        let mut m = parse(&format!("{EXAMPLE}\n{EXTERNAL_AGENT}")).expect("parses");
        // 64 on its own, but not after `git-glance-`.
        m.external_agents[0].name = "c".repeat(64);
        assert_eq!(m.validate(), Err(ManifestError::ToolName("c".repeat(64))));
    }
}
