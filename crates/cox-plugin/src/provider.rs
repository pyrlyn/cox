// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Plugin providers (PL§7a). Declarative `api = "chat" | "responses"` rows
//! merge into `providers.custom` so cox drives them with its own
//! OpenAI-shaped clients — no wasm on that path ([`merge`], T33.17).
//! `api = "plugin"` is [`PluginProvider`] (T33.18): `stream` calls
//! `cox_provider_stream` and forwards `ProviderEvent`s. The guest's
//! `cox_http` reaches only the section's host, and the auth header is
//! injected on the host request so the key never enters wasm. Token
//! estimation is injected by the caller: this crate cannot depend on
//! `cox-tokens`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use cox_plugin_api::ProviderCall;
use cox_protocol::config::{CompatibleProviderConfig, ProvidersConfig};
use cox_protocol::errors::ProviderError;
use cox_protocol::plugin::{ProviderApi, ProviderAuth, ProviderDecl};
use cox_protocol::traits::Provider;
use cox_protocol::types::{Caps, ProviderEvent, ProviderId, Request, Usage};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::hostfn::HostEnv;
use crate::live::LivePlugins;
use crate::{Lane, PluginError, PluginHost};

/// One granted plugin's `[[provider]]` rows, as [`merge`] takes them.
/// Borrowed from the manifest, like `cox_models::catalog::PluginModels`
/// (T33.16), so this crate needs no store or plugin host to decide the
/// merge.
#[derive(Debug, Clone, Copy)]
pub struct PluginProviders<'a> {
    /// The plugin id: the tie-break between two plugins claiming the same
    /// new section name, and named in every skipped-section warning.
    pub plugin: &'a str,
    /// Its `[[provider]]` rows.
    pub decls: &'a [ProviderDecl],
}

/// [`merge`]'s result.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Merged {
    /// New `providers.custom` entries the caller should add to the live
    /// config. A section name a config (or a native `[providers.*]` table)
    /// already uses is never in here.
    pub custom: HashMap<String, CompatibleProviderConfig>,
    /// One line per `[[provider]]` row that was skipped, for
    /// `Notice(Warn)` — fail open on extensions (AGENTS.md "Trust
    /// boundaries").
    pub warnings: Vec<String>,
}

/// `ProvidersConfig`'s own named sections: each carries knobs
/// (`cache_ttl`, `fallbacks`, `load`, …) a `CompatibleProviderConfig`
/// cannot express, so a plugin can never take one of these names — only
/// `providers.custom`'s flattened map, which is exactly what [`merge`]
/// fills.
const RESERVED: &[&str] = &["anthropic", "openai", "local", "lmstudio", "typesafe"];

/// Builds the plugin layer of `providers.custom` (PL§7a, T33.17): each
/// granted plugin's `api = "chat" | "responses"` `[[provider]]` row becomes
/// one more compatible section, unless a user config section of the same
/// name already exists (native or `custom`) — that wins, and the plugin's
/// section is skipped with a notice, exactly as the design doc says. A
/// name two plugins both claim goes to the lower plugin id, so the order
/// `plugins` is given in never decides the winner — the same rule
/// `cox_models::catalog::PluginModels` uses for a contested model id
/// (T33.16).
pub fn merge(providers: &ProvidersConfig, plugins: &[PluginProviders<'_>]) -> Merged {
    let mut ordered: Vec<&PluginProviders<'_>> = plugins.iter().collect();
    ordered.sort_by_key(|p| p.plugin);
    let mut out = Merged::default();
    let mut owners: HashMap<String, String> = HashMap::new();
    for p in ordered {
        for decl in p.decls {
            merge_one(providers, p.plugin, decl, &mut out, &mut owners);
        }
    }
    out
}

fn merge_one(
    providers: &ProvidersConfig,
    plugin: &str,
    decl: &ProviderDecl,
    out: &mut Merged,
    owners: &mut HashMap<String, String>,
) {
    let api = match decl.api {
        ProviderApi::Chat => "chat",
        ProviderApi::Responses => "responses",
        // The ABI form has no wire client here; T33.18's `PluginProvider`
        // drives `api = "plugin"` outside `providers.custom`.
        ProviderApi::Plugin => return,
    };
    if RESERVED.contains(&decl.name.as_str()) || providers.custom.contains_key(&decl.name) {
        out.warnings.push(format!(
            "plugin {plugin}'s provider section `{}` is shadowed by an existing `providers.{}` config section",
            decl.name, decl.name
        ));
        return;
    }
    if let Some(owner) = owners.get(&decl.name) {
        out.warnings.push(format!(
            "plugin {plugin} also declares provider {}; plugin {owner}'s section is kept",
            decl.name
        ));
        return;
    }
    if decl.auth == ProviderAuth::XApiKey {
        // The declarative chat/responses wire only ever sends
        // `Authorization: Bearer <key>` (`crates/cox-provider-openai`); a
        // section that needs `x-api-key` must use the ABI form (T33.18)
        // instead of silently getting the wrong header.
        out.warnings.push(format!(
            "plugin {plugin}'s provider section `{}` asks for x-api-key auth, which the declarative chat/responses client cannot send; skipped",
            decl.name
        ));
        return;
    }
    owners.insert(decl.name.clone(), plugin.to_string());
    out.custom.insert(
        decl.name.clone(),
        CompatibleProviderConfig {
            base_url: decl.base_url.clone(),
            api_key_env: decl.api_key_env.clone().unwrap_or_default(),
            api: api.to_string(),
            ..CompatibleProviderConfig::default()
        },
    );
}

/// An ABI provider: one `[[provider]]` section with `api = "plugin"`.
pub struct PluginProvider {
    section: String,
    host: Arc<PluginHost>,
    env: Arc<HostEnv>,
    estimate: Arc<dyn Fn(&Request) -> u32 + Send + Sync>,
    warned: AtomicBool,
}

impl PluginProvider {
    pub(crate) fn new(
        section: impl Into<String>,
        host: Arc<PluginHost>,
        env: Arc<HostEnv>,
        estimate: Arc<dyn Fn(&Request) -> u32 + Send + Sync>,
    ) -> Self {
        Self {
            section: section.into(),
            host,
            env,
            estimate,
            warned: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl Provider for PluginProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Plugin(self.section.clone())
    }

    fn capabilities(&self) -> Caps {
        Caps {
            cache: false,
            thinking: false,
            server_tools: false,
            count_tokens: false,
            max_context: 128_000,
        }
    }

    async fn stream(
        &self,
        req: Request,
        sink: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<Usage, ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let estimate = (self.estimate)(&req);
        let call = ProviderCall {
            request: serde_json::to_value(&req).map_err(|e| ProviderError::BadRequest {
                message: e.to_string(),
            })?,
            model: req.model.0.clone(),
        };
        let host = Arc::clone(&self.host);
        let env = Arc::clone(&self.env);
        let section = self.section.clone();
        let events = tokio::task::spawn_blocking(move || {
            let _guard = env.activate_provider(&section);
            match host.call::<_, Vec<ProviderEvent>>(
                Lane::Control,
                "cox_provider_stream",
                &call,
                Duration::from_secs(60),
            ) {
                Ok(Some(events)) => Ok(events),
                Ok(None) => Err(ProviderError::Unsupported {
                    feature: "cox_provider_stream".into(),
                }),
                Err(error) => Err(map_plugin(error)),
            }
        })
        .await
        .map_err(|_| ProviderError::BadRequest {
            message: "plugin worker stopped".into(),
        })??;
        let mut forwarded = None;
        for event in events {
            if cancel.is_cancelled() {
                return Err(ProviderError::Cancelled);
            }
            let event = match event {
                ProviderEvent::Usage { usage } => {
                    let usage = floor_usage(Some(usage), estimate, &self.warned);
                    forwarded = Some(usage);
                    ProviderEvent::Usage { usage }
                }
                other => other,
            };
            if sink.send(event).await.is_err() {
                return Err(ProviderError::Cancelled);
            }
        }
        let usage = match forwarded {
            Some(usage) => usage,
            None => {
                let usage = floor_usage(None, estimate, &self.warned);
                if sink.send(ProviderEvent::Usage { usage }).await.is_err() {
                    return Err(ProviderError::Cancelled);
                }
                usage
            }
        };
        Ok(usage)
    }

    async fn count_tokens(&self, req: &Request) -> Result<u32, ProviderError> {
        Ok((self.estimate)(req))
    }
}

/// Builds one [`PluginProvider`] per granted `api = "plugin"` section.
/// Lower plugin id wins a contested name. A missing key is a warning and
/// a keyless client, not a failed session (fail open).
pub fn backends(
    live: &LivePlugins,
    resolve: &dyn Fn(&str, &str) -> Result<String, ProviderError>,
    estimate: Arc<dyn Fn(&Request) -> u32 + Send + Sync>,
) -> (HashMap<String, Arc<dyn Provider>>, Vec<String>) {
    let mut plugins: Vec<_> = live.plugins().iter().collect();
    plugins.sort_by(|a, b| a.id().cmp(b.id()));
    let mut out = HashMap::new();
    let mut owners: HashMap<String, String> = HashMap::new();
    let mut warnings = Vec::new();
    for plugin in plugins {
        for decl in plugin.provider_decls() {
            if decl.api != ProviderApi::Plugin {
                continue;
            }
            if let Some(owner) = owners.get(&decl.name) {
                warnings.push(format!(
                    "plugin {} also declares provider {}; plugin {owner}'s section is kept",
                    plugin.id(),
                    decl.name
                ));
                continue;
            }
            let header = auth_header(plugin.id(), decl, resolve, &mut warnings);
            if !plugin
                .env()
                .bind_provider(&decl.name, &decl.base_url, header)
            {
                warnings.push(format!(
                    "plugin {}'s provider section `{}` has no usable base_url; skipped",
                    plugin.id(),
                    decl.name
                ));
                continue;
            }
            owners.insert(decl.name.clone(), plugin.id().to_string());
            let provider = Arc::new(PluginProvider::new(
                decl.name.clone(),
                Arc::clone(plugin.host()),
                Arc::clone(plugin.env()),
                Arc::clone(&estimate),
            ));
            out.insert(decl.name.clone(), provider as Arc<dyn Provider>);
        }
    }
    (out, warnings)
}

fn auth_header(
    plugin: &str,
    decl: &ProviderDecl,
    resolve: &dyn Fn(&str, &str) -> Result<String, ProviderError>,
    warnings: &mut Vec<String>,
) -> Option<(String, String)> {
    if decl.auth == ProviderAuth::None {
        return None;
    }
    let Some(var) = decl.api_key_env.as_deref().filter(|var| !var.is_empty()) else {
        warnings.push(format!(
            "plugin {plugin}'s provider section `{}` has no api_key_env; built without auth",
            decl.name
        ));
        return None;
    };
    let key = match resolve(var, &decl.name) {
        Ok(key) => key,
        Err(_) => {
            warnings.push(format!(
                "plugin {plugin}'s provider section `{}` has no API key; built without auth",
                decl.name
            ));
            return None;
        }
    };
    match decl.auth {
        ProviderAuth::Bearer => Some(("authorization".into(), format!("Bearer {key}"))),
        ProviderAuth::XApiKey => Some(("x-api-key".into(), key)),
        ProviderAuth::None => None,
    }
}

/// Missing usage becomes the estimate. Reported input below half of it is
/// replaced, once per provider, and flagged `estimated`.
fn floor_usage(reported: Option<Usage>, estimate: u32, warned: &AtomicBool) -> Usage {
    let Some(reported) = reported else {
        return Usage {
            input_tokens: estimate,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            estimated: true,
            cost_usd: 0.0,
            latency_ms: 0,
        };
    };
    if reported.input_tokens >= estimate / 2 {
        return reported;
    }
    if !warned.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            reported = reported.input_tokens,
            estimate,
            "plugin provider reported input tokens below half of cox's estimate; the estimate replaces them"
        );
    }
    Usage {
        input_tokens: estimate,
        estimated: true,
        ..reported
    }
}

fn map_plugin(error: PluginError) -> ProviderError {
    match error {
        PluginError::Timeout { .. } => ProviderError::Timeout,
        PluginError::MissingExport(_) => ProviderError::Unsupported {
            feature: "cox_provider_stream".into(),
        },
        other => ProviderError::BadRequest {
            message: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(name: &str, api: ProviderApi) -> ProviderDecl {
        ProviderDecl {
            name: name.into(),
            api,
            base_url: "https://api.example.test".into(),
            api_key_env: Some("EXAMPLE_API_KEY".into()),
            auth: ProviderAuth::Bearer,
        }
    }

    #[test]
    fn new_section_merges_as_compatible_config() {
        let decls = [decl("acme", ProviderApi::Chat)];
        let plugins = [PluginProviders {
            plugin: "acme-pkg",
            decls: &decls,
        }];
        let merged = merge(&ProvidersConfig::default(), &plugins);
        assert!(merged.warnings.is_empty(), "{:?}", merged.warnings);
        let section = merged.custom.get("acme").expect("merged section");
        assert_eq!(section.base_url, "https://api.example.test");
        assert_eq!(section.api_key_env, "EXAMPLE_API_KEY");
        assert_eq!(section.api, "chat");
    }

    #[test]
    fn config_section_shadows_plugin_section() {
        let decls = [decl("acme", ProviderApi::Chat)];
        let plugins = [PluginProviders {
            plugin: "acme-pkg",
            decls: &decls,
        }];
        let mut providers = ProvidersConfig::default();
        providers
            .custom
            .insert("acme".into(), CompatibleProviderConfig::default());
        let merged = merge(&providers, &plugins);
        assert!(
            merged.custom.is_empty(),
            "the plugin section is skipped, not merged"
        );
        assert_eq!(
            merged.warnings,
            [
                "plugin acme-pkg's provider section `acme` is shadowed by an existing `providers.acme` config section"
            ]
        );
    }

    #[test]
    fn reserved_native_names_are_never_taken() {
        let decls = [decl("anthropic", ProviderApi::Chat)];
        let plugins = [PluginProviders {
            plugin: "acme-pkg",
            decls: &decls,
        }];
        let merged = merge(&ProvidersConfig::default(), &plugins);
        assert!(merged.custom.is_empty());
        assert_eq!(merged.warnings.len(), 1, "{:?}", merged.warnings);
    }

    #[test]
    fn duplicate_plugin_names_lower_id_wins() {
        let alpha_decls = [decl("shared", ProviderApi::Chat)];
        let zeta_decls = [ProviderDecl {
            base_url: "https://other.test".into(),
            ..decl("shared", ProviderApi::Chat)
        }];
        let plugins = [
            PluginProviders {
                plugin: "zeta",
                decls: &zeta_decls,
            },
            PluginProviders {
                plugin: "alpha",
                decls: &alpha_decls,
            },
        ];
        let merged = merge(&ProvidersConfig::default(), &plugins);
        let section = merged.custom.get("shared").expect("kept section");
        assert_eq!(
            section.base_url, "https://api.example.test",
            "alpha's row, not zeta's"
        );
        assert_eq!(
            merged.warnings,
            ["plugin zeta also declares provider shared; plugin alpha's section is kept"]
        );
    }

    #[test]
    fn plugin_api_form_is_left_to_t33_18() {
        let decls = [decl("typesafe-guest", ProviderApi::Plugin)];
        let plugins = [PluginProviders {
            plugin: "jev",
            decls: &decls,
        }];
        let merged = merge(&ProvidersConfig::default(), &plugins);
        assert!(merged.custom.is_empty());
        assert!(
            merged.warnings.is_empty(),
            "silently out of scope, not an error"
        );
    }

    #[test]
    fn x_api_key_auth_is_unsupported_declaratively() {
        let decls = [ProviderDecl {
            auth: ProviderAuth::XApiKey,
            ..decl("acme", ProviderApi::Chat)
        }];
        let plugins = [PluginProviders {
            plugin: "acme-pkg",
            decls: &decls,
        }];
        let merged = merge(&ProvidersConfig::default(), &plugins);
        assert!(merged.custom.is_empty());
        assert_eq!(merged.warnings.len(), 1);
        assert!(
            merged.warnings[0].contains("x-api-key"),
            "{:?}",
            merged.warnings
        );
    }

    #[test]
    fn missing_api_key_env_builds_a_keyless_section() {
        let decls = [ProviderDecl {
            api_key_env: None,
            ..decl("acme", ProviderApi::Responses)
        }];
        let plugins = [PluginProviders {
            plugin: "acme-pkg",
            decls: &decls,
        }];
        let merged = merge(&ProvidersConfig::default(), &plugins);
        let section = merged.custom.get("acme").expect("merged section");
        assert_eq!(section.api_key_env, "");
        assert_eq!(section.api, "responses");
    }

    use cox_plugin_api::Limits;
    use cox_protocol::types::{
        Content, Effort, Message, ModelId, Role, SystemBlock, Thinking, Tier,
    };
    use tokio_util::sync::CancellationToken;

    use crate::host::PluginHost;

    fn kernel() -> &'static str {
        r#"
          (import "extism:host/env" "http_request" (func $http (param i64 i64) (result i64)))
          (import "extism:host/env" "input_length" (func $input_length (result i64)))
          (import "extism:host/env" "input_load_u8" (func $load (param i64) (result i32)))
          (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
          (import "extism:host/env" "store_u8" (func $store (param i64 i32)))
          (import "extism:host/env" "output_set" (func $output_set (param i64 i64)))
          (import "cox:host/v1" "cox_http" (func $cox_http (param i64) (result i64)))"#
    }

    fn escape(json: &str) -> String {
        json.replace('\\', "\\\\").replace('"', "\\\"")
    }

    fn answering(json: &str) -> Vec<u8> {
        let escaped = escape(json);
        format!(
            r#"(module {kernel}
               (memory 1)
               (data (i32.const 0) "{escaped}")
               (func (export "cox_provider_stream") (result i32) (local $off i64) (local $i i64)
                 (local.set $off (call $alloc (i64.const {len})))
                 (block $done (loop $copy
                   (br_if $done (i64.ge_u (local.get $i) (i64.const {len})))
                   (call $store (i64.add (local.get $off) (local.get $i))
                     (i32.load8_u (i32.wrap_i64 (local.get $i))))
                   (local.set $i (i64.add (local.get $i) (i64.const 1)))
                   (br $copy)))
                 (call $output_set (local.get $off) (i64.const {len}))
                 (i32.const 0))
               (func (export "cox_init") (result i32) (i32.const 0)))"#,
            kernel = kernel(),
            len = json.len()
        )
        .into_bytes()
    }

    /// Calls `cox_http` with the guest's own request, then returns events
    /// whose text is that same request. The key is not in either.
    fn http_then_echo(http: &str, events: &str) -> Vec<u8> {
        format!(
            r#"(module {kernel}
               (memory 1)
               (data (i32.const 0) "{http}")
               (data (i32.const {http_len}) "{events}")
               (func (export "cox_provider_stream") (result i32)
                 (local $off i64) (local $i i64)
                 (local.set $off (call $alloc (i64.const {http_len})))
                 (block $done (loop $copy
                   (br_if $done (i64.ge_u (local.get $i) (i64.const {http_len})))
                   (call $store (i64.add (local.get $off) (local.get $i))
                     (i32.load8_u (i32.wrap_i64 (local.get $i))))
                   (local.set $i (i64.add (local.get $i) (i64.const 1)))
                   (br $copy)))
                 (drop (call $cox_http (local.get $off)))
                 (local.set $i (i64.const 0))
                 (local.set $off (call $alloc (i64.const {ev_len})))
                 (block $done2 (loop $copy2
                   (br_if $done2 (i64.ge_u (local.get $i) (i64.const {ev_len})))
                   (call $store (i64.add (local.get $off) (local.get $i))
                     (i32.load8_u offset={http_len} (i32.wrap_i64 (local.get $i))))
                   (local.set $i (i64.add (local.get $i) (i64.const 1)))
                   (br $copy2)))
                 (call $output_set (local.get $off) (i64.const {ev_len}))
                 (i32.const 0))
               (func (export "cox_init") (result i32) (i32.const 0)))"#,
            kernel = kernel(),
            http = escape(http),
            events = escape(events),
            http_len = http.len(),
            ev_len = events.len(),
        )
        .into_bytes()
    }

    fn sample_request() -> Request {
        Request {
            tier: Tier::Code,
            job: cox_protocol::types::Job::Main,
            model: ModelId("acme-model".into()),
            system: vec![SystemBlock {
                text: "you are a plugin provider".into(),
                cache: false,
            }],
            tools: vec![],
            messages: vec![Message {
                role: Role::User,
                content: vec![Content::Text {
                    text: "hello".into(),
                }],
            }],
            effort: Effort::High,
            max_tokens: 32,
            thinking: Thinking::Off,
            cache_breakpoints: vec![],
            stop_sequences: vec![],
        }
    }

    fn usage_json(input: u32, output: u32) -> serde_json::Value {
        serde_json::json!({
            "input_tokens": input,
            "output_tokens": output,
            "cache_read_tokens": 0,
            "cache_write_tokens": 0,
            "estimated": false,
            "cost_usd": 0.0,
            "latency_ms": 1
        })
    }

    async fn drive(
        wasm: Vec<u8>,
        section: &str,
        base: &str,
        header: Option<(String, String)>,
        estimate: u32,
    ) -> (Usage, Vec<ProviderEvent>) {
        let mut env = HostEnv::new(section);
        env.set_runtime(tokio::runtime::Handle::current());
        let env = Arc::new(env);
        assert!(
            env.bind_provider(section, base, header),
            "section host binds"
        );
        let host = Arc::new(
            PluginHost::load_with(
                &wasm,
                &Limits {
                    call_ms: Some(10_000),
                    ..Limits::default()
                },
                Arc::clone(&env),
            )
            .expect("module loads"),
        );
        let provider =
            PluginProvider::new(section, host, env, Arc::new(move |_: &Request| estimate));
        let (tx, mut rx) = mpsc::channel(16);
        let usage = provider
            .stream(sample_request(), tx, CancellationToken::new())
            .await
            .expect("stream");
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        (usage, events)
    }

    fn forwarded_usage(events: &[ProviderEvent]) -> Usage {
        events
            .iter()
            .rev()
            .find_map(|event| match event {
                ProviderEvent::Usage { usage } => Some(*usage),
                _ => None,
            })
            .expect("a usage event")
    }

    #[tokio::test]
    async fn provider_key_never_reaches_guest() {
        use wiremock::matchers::{header, method};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let secret = "cox-plugin-key-do-not-leak";
        Mock::given(method("POST"))
            .and(header("authorization", format!("Bearer {secret}")))
            .and(header("x-echo", "from-guest"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .expect(1)
            .mount(&server)
            .await;
        let http = serde_json::json!({
            "method": "POST",
            "url": format!("{}/v1/messages", server.uri()),
            "headers": { "x-echo": "from-guest", "authorization": "Bearer from-guest" },
            "body": "{}"
        });
        let http_json = serde_json::to_string(&http).expect("http json");
        let events = serde_json::json!([
            {"type": "text_delta", "text": http_json},
            {"type": "stop", "stop": {"type": "end_turn"}},
            {"type": "usage", "usage": usage_json(8, 1)}
        ]);
        let events_json = serde_json::to_string(&events).expect("events json");
        let (_usage, got) = drive(
            http_then_echo(&http_json, &events_json),
            "acme",
            &server.uri(),
            Some(("authorization".into(), format!("Bearer {secret}"))),
            8,
        )
        .await;
        let rendered = serde_json::to_string(&got).expect("events");
        assert!(
            !rendered.contains(secret),
            "the key reached the guest output: {rendered}"
        );
        assert!(rendered.contains("from-guest"));
        server.verify().await;
    }

    #[tokio::test]
    async fn underreported_usage_is_replaced_by_estimate() {
        let events = serde_json::json!([
            {"type": "usage", "usage": usage_json(1, 4)},
            {"type": "stop", "stop": {"type": "end_turn"}}
        ]);
        let (usage, got) = drive(
            answering(&events.to_string()),
            "acme",
            "https://llm.example",
            None,
            100,
        )
        .await;
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 4);
        assert!(usage.estimated);
        let forwarded = forwarded_usage(&got);
        assert_eq!(forwarded.input_tokens, 100);
        assert!(forwarded.estimated);

        // At half the estimate the guest's figure stands.
        let events = serde_json::json!([
            {"type": "usage", "usage": usage_json(50, 4)}
        ]);
        let (usage, _) = drive(
            answering(&events.to_string()),
            "acme",
            "https://llm.example",
            None,
            100,
        )
        .await;
        assert_eq!(usage.input_tokens, 50);
        assert!(!usage.estimated);

        // No usage event: the estimate fills the row and is forwarded.
        let (usage, got) = drive(
            answering(r#"[{"type":"stop","stop":{"type":"end_turn"}}]"#),
            "acme",
            "https://llm.example",
            None,
            100,
        )
        .await;
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 0);
        assert!(usage.estimated);
        assert_eq!(forwarded_usage(&got), usage);
    }

    #[tokio::test]
    async fn every_request_has_a_usage_row() {
        use std::path::PathBuf;

        use cox_core::{MemoryStore, Session};
        use cox_protocol::types::Submission;

        let events = serde_json::json!([
            {"type": "text_delta", "text": "hello"},
            {"type": "stop", "stop": {"type": "end_turn"}}
        ]);
        let mut env = HostEnv::new("acme");
        env.set_runtime(tokio::runtime::Handle::current());
        let env = Arc::new(env);
        assert!(env.bind_provider("acme", "https://llm.example", None));
        let host = Arc::new(
            PluginHost::load_with(
                &answering(&events.to_string()),
                &Limits {
                    call_ms: Some(10_000),
                    ..Limits::default()
                },
                Arc::clone(&env),
            )
            .expect("module loads"),
        );
        let provider = Arc::new(PluginProvider::new(
            "acme",
            host,
            env,
            Arc::new(|_: &Request| 12),
        ));
        let store = Arc::new(MemoryStore::new());
        let mut config = cox_protocol::Config::default();
        config.core.workspace_roots = vec![PathBuf::from("/tmp/cox-plugin-provider")];
        let session = Session::new(
            config,
            provider,
            Vec::new(),
            Arc::clone(&store) as Arc<dyn cox_protocol::traits::Store>,
            store.clone(),
            PathBuf::from("/tmp/cox-plugin-provider"),
        )
        .expect("session");
        let mut rx = session.events().expect("events");
        let running = session.clone();
        let turn = tokio::spawn(async move {
            running
                .submit(Submission::UserTurn {
                    text: "hello".into(),
                    attachments: vec![],
                    confirm_think: false,
                })
                .await
        });
        loop {
            let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("event in time")
                .expect("stream open");
            if matches!(event, cox_protocol::types::Event::TurnDone { .. }) {
                break;
            }
        }
        turn.await.expect("join").expect("turn");
        let rows = store.usage_rows();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].provider, ProviderId::Plugin("acme".into()));
        assert!(rows[0].usage.estimated);
        assert_eq!(rows[0].usage.input_tokens, 12);
    }
}
