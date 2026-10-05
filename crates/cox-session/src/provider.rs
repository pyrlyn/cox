// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The provider a session talks to (T9.1, T30.15, T30.16, T30.23): the
//! `tiers.code` section picks the wire client, the model catalog its
//! context window, and every real client is wrapped in `Priced` so each
//! call reaches the ledger. Separate from `open` because ACP and `doctor`
//! build or ask about the same provider without a session.

use std::sync::Arc;

use cox_protocol::Config;
use cox_protocol::config::Transport;
use cox_protocol::traits::Provider;
use cox_provider::anthropic::{AnthropicProvider, CacheTtl};
use cox_provider::openai::chat::OpenAiChatProvider;
use cox_provider::openai::responses::OpenAiResponsesProvider;
use cox_provider::usage::{PriceTable, Priced};

use crate::SessionError;

/// The `tiers.code` provider decides which real client to build; every tier
/// of a session goes through the same provider object (routing picks models).
/// Real clients are wrapped in `Priced` so every call reaches the ledger with
/// its cost; test doubles are not, because their scenarios script the cost.
pub fn provider_for(config: &Config) -> Result<Arc<dyn Provider>, SessionError> {
    provider_for_with(config, cox_provider::http::resolve_key)
}

/// [`provider_for`]'s body with the credential lookup injected, so a test
/// can build every provider kind without ever reaching the real keyring
/// (A49, T30.28).
fn provider_for_with(
    config: &Config,
    resolve: impl FnOnce(&str, &str) -> Result<String, cox_protocol::errors::ProviderError>,
) -> Result<Arc<dyn Provider>, SessionError> {
    provider_for_served(config, resolve, None, &[])
}

/// [`provider_for_with`] plus what a local server reported for the
/// session's model (T30.16), overlaid on the catalog before any lookup.
pub(crate) fn provider_for_served(
    config: &Config,
    resolve: impl FnOnce(&str, &str) -> Result<String, cox_protocol::errors::ProviderError>,
    served: Option<&cox_provider::lmstudio::Model>,
    plugin_models: &[cox_models::PluginModels<'_>],
) -> Result<Arc<dyn Provider>, SessionError> {
    if let Some(double) = cox_provider::from_env()? {
        return Ok(Arc::from(double));
    }
    let prices = Arc::new(PriceTable::embedded()?);
    Ok(Arc::new(Priced::new(
        backend_for_with(config, resolve, served, plugin_models)?,
        prices,
    )))
}

/// The real client `tiers.code.provider` names, before pricing: one lookup
/// from the section to (api shape, `&Transport`) to a constructor (T30.23),
/// instead of a bespoke arm per family. Anthropic and Jev stay their own
/// arms because they carry section-specific knobs no OpenAI-shaped section
/// has (cache TTL/fallbacks; a decision model); every `api = "chat"` or
/// `"responses"` section — native `openai`/`local` and any Type-2 compatible
/// section alike — goes through the one `openai_shaped` constructor.
///
/// Takes the credential lookup as `resolve` (A49, T30.28): every arm
/// resolves its key through it — `AnthropicProvider::with_key`/
/// `JevProvider::with_key` take the already-resolved key instead of
/// resolving it themselves, and `openai_shaped` takes `resolve` straight
/// through — so [`provider_for_with`]'s caller decides whether that is the
/// real `cox_provider::http::resolve_key` or a test's fake lookup.
fn backend_for_with(
    config: &Config,
    resolve: impl FnOnce(&str, &str) -> Result<String, cox_protocol::errors::ProviderError>,
    served: Option<&cox_provider::lmstudio::Model>,
    plugin_models: &[cox_models::PluginModels<'_>],
) -> Result<Arc<dyn Provider>, SessionError> {
    // T30.25: `Caps.max_context` for the sections below comes from the
    // model catalog rather than a per-family literal. `Catalog::load`
    // always carries the embedded built-in rows even when `config` overlays
    // none of its own (unlike reading `providers.*.models` directly, which
    // is empty on a bare `Config::default()`); a bad/unparseable catalog
    // falls back to the empty default, which is exactly "no row found" —
    // every lookup below already has its own literal fallback for that.
    // `plugin_models` are the granted plugins' `[[models]]` (T33.44).
    let mut catalog = cox_models::Catalog::load(config, plugin_models, None).unwrap_or_default();
    // T30.16 (A46): a server's own report is the last override layer.
    if let Some(m) = served {
        catalog.overlay_served(&m.key, m.loaded_context(), m.tool_use());
    }
    match config.tiers.code.provider.as_str() {
        "anthropic" => {
            let a = &config.providers.anthropic;
            let ttl = match a.cache_ttl.as_str() {
                "1h" => CacheTtl::OneHour,
                _ => CacheTtl::FiveMinutes,
            };
            let transport = a.transport();
            let api_key = resolve(&transport.api_key_env, "anthropic")?;
            // 200k, same as before T30.25, when the catalog has no row for
            // the tier's configured model (e.g. a custom id absent from
            // both the built-in and configured `models` lists).
            let max_context = catalog
                .get(&config.tiers.code.model)
                .and_then(|row| row.context_window)
                .unwrap_or(200_000);
            Ok(Arc::new(AnthropicProvider::with_key(
                &transport,
                Some(api_key),
                ttl,
                a.fallbacks,
                max_context,
            )?))
        }
        // T30.15: LM Studio's Anthropic-compatible `/v1/messages` goes
        // through the same Anthropic wire client as the "anthropic" arm
        // above — its native `/api/v1/chat` takes no custom tool schemas
        // and cox's OpenAI Chat path drops tool calls (R§4.3.2), so
        // Messages is the one working chat transport. The key resolves
        // under this section's own name ("lmstudio"), so it never falls
        // back to the Anthropic keyring entry; missing it builds keyless
        // (T30.21 shape), which is what LM Studio needs unless "Require
        // Authentication" is on.
        "lmstudio" => {
            let l = &config.providers.lmstudio;
            let transport = l.transport();
            let api_key = resolve(&transport.api_key_env, "lmstudio").ok();
            // `context_window = 0` means "ask the server": its loaded
            // context, overlaid on the catalog above (T30.16), then the
            // catalog's own row, then the same literal floor `local` uses.
            let max_context = if l.context_window > 0 {
                l.context_window
            } else {
                catalog
                    .get(lmstudio_model(config))
                    .and_then(|row| row.context_window)
                    .unwrap_or(32_768)
            };
            Ok(Arc::new(AnthropicProvider::with_key(
                &transport,
                api_key,
                CacheTtl::FiveMinutes,
                // The `fallbacks: "default"` beta is Anthropic's own
                // server-side model fallback; meaningless against LM
                // Studio, so this arm never sends it.
                false,
                max_context,
            )?))
        }
        // Jev is type-1 native (System One wire, T21.1): its own client,
        // not an OpenAI shape. A missing key fails `Auth` rather than
        // building keyless — that is the fail-open path, read as auth, not
        // transport.
        "typesafe" => {
            let t = &config.providers.typesafe;
            let transport = t.transport();
            let api_key = resolve(&transport.api_key_env, "typesafe")?;
            // 128k, same as before T30.25, when the catalog has no row —
            // expected, since Jev/TypeSafe models have no models.dev
            // counterpart (`cox-vendor models` never touches this section).
            let max_context = catalog
                .get(&t.model)
                .and_then(|row| row.context_window)
                .unwrap_or(128_000);
            Ok(Arc::new(cox_provider::jev::JevProvider::with_key(
                &transport,
                api_key,
                t.model.clone(),
                max_context,
            )?))
        }
        "openai" => {
            let o = &config.providers.openai;
            // No `context_window` field on the native section (it relies
            // on `models`); 400k is the same fallback `openai_shaped` used
            // before this lookup existed, now reached only when the
            // catalog has no row for the tier's configured model either.
            let max_context = catalog
                .get(&config.tiers.code.model)
                .and_then(|row| row.context_window)
                .unwrap_or(400_000);
            openai_shaped(
                "openai",
                &o.transport(),
                o.models.clone(),
                max_context,
                &o.api,
                resolve,
            )
        }
        "local" => {
            let l = &config.providers.local;
            openai_shaped(
                "local",
                &l.transport(),
                l.models.clone(),
                l.context_window,
                &l.api,
                resolve,
            )
        }
        // Type-2 providers: no code per vendor — the section's `api` picks
        // the wire client, the section's transport/key/models configure it.
        other => {
            let c = config
                .providers
                .custom
                .get(other)
                .ok_or_else(|| SessionError::UnknownProvider(other.to_string()))?;
            openai_shaped(
                other,
                &c.transport(),
                c.models.clone(),
                c.context_window,
                &c.api,
                resolve,
            )
        }
    }
}

/// Builds the OpenAI-shaped client the `api` string names for `owner`:
/// `"responses"` speaks the Responses API, `"chat"` the Chat Completions
/// subset every compatible vendor speaks. Anything else is a config error
/// at startup, not a mid-turn 404. The key resolves once, here, through
/// `resolve` — `transport.api_key_env` first, else the keyring entry
/// `cox/<owner>` for the real caller (`backend_for`); missing both builds
/// keyless (no `Authorization` header) rather than failing at startup
/// (T30.21) — most compatible sections, and every local/self-hosted
/// gateway, need no key at all.
fn openai_shaped(
    owner: &str,
    transport: &Transport,
    models: Vec<cox_protocol::config::ProviderModel>,
    context_window: u32,
    api: &str,
    resolve: impl FnOnce(&str, &str) -> Result<String, cox_protocol::errors::ProviderError>,
) -> Result<Arc<dyn Provider>, SessionError> {
    let api_key = resolve(&transport.api_key_env, owner).ok();
    match api {
        "responses" => Ok(Arc::new(OpenAiResponsesProvider::new(
            transport,
            api_key,
            models,
            context_window,
        )?)),
        "chat" => Ok(Arc::new(OpenAiChatProvider::new(
            transport,
            api_key,
            models,
            context_window,
        )?)),
        _ => Err(SessionError::UnknownApi {
            api: api.to_string(),
            owner: owner.to_string(),
        }),
    }
}

/// The model id an `lmstudio` session sends: the section's pin, else
/// `tiers.code.model` — `Router::pick`'s rule, so the id asked about is the
/// id chatted with.
pub fn lmstudio_model(config: &Config) -> &str {
    let pinned = &config.providers.lmstudio.model;
    if pinned.is_empty() {
        &config.tiers.code.model
    } else {
        pinned
    }
}

/// What LM Studio reported for the session's model, plus the key it was
/// asked with.
pub(crate) struct Served {
    pub(crate) model: cox_provider::lmstudio::Model,
    pub(crate) api_key: Option<String>,
}

/// T30.16: reads (and, with `load = true`, loads) the session's model on
/// LM Studio's native API. `None` unless `tiers.code` is `lmstudio` and no
/// test double (`COX_PROVIDER`) stands in for the server. An unreachable
/// server is an error here, before any turn: the first chat call would
/// fail the same way.
pub(crate) async fn lmstudio_served(config: &Config) -> Result<Option<Served>, SessionError> {
    let double = std::env::var_os("COX_PROVIDER").is_some_and(|v| !v.is_empty());
    if config.tiers.code.provider != "lmstudio" || double {
        return Ok(None);
    }
    let l = &config.providers.lmstudio;
    let transport = l.transport();
    let api_key = cox_provider::http::resolve_key(&transport.api_key_env, "lmstudio").ok();
    let client = cox_provider::lmstudio::LmStudio::new(&transport, api_key.clone())?;
    let context_length = (l.context_window > 0).then_some(l.context_window);
    let model = client
        .prepare(lmstudio_model(config), l.load, context_length)
        .await
        .map_err(|error| SessionError::LmStudio {
            base_url: transport.base_url.clone(),
            error,
        })?;
    Ok(Some(Served { model, api_key }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use cox_protocol::config::CompatibleProviderConfig;
    use cox_protocol::types::ProviderId;
    #[cfg(feature = "plugins")]
    use {
        crate::testing::user_turn, cox_core::Session, cox_protocol::traits::Store as _,
        cox_protocol::types::Job, cox_store::Store,
    };

    fn deepseek_config(api: &str) -> Config {
        let mut cfg = Config::default();
        cfg.tiers.code.provider = "deepseek".into();
        cfg.providers.custom.insert(
            "deepseek".into(),
            CompatibleProviderConfig {
                base_url: "https://api.deepseek.com".into(),
                // Never resolved in a test (A49, T30.28): every caller below
                // goes through `provider_for_with` with a fake resolver, so
                // this name is never looked up anywhere, real or fake.
                api_key_env: "COX_TEST_MISSING_KEY_DEEPSEEK".into(),
                api: api.into(),
                model: "deepseek-v4-pro".into(),
                context_window: 1_000_000,
                models: vec![],
                ..Default::default()
            },
        );
        cfg
    }

    /// A49 (T30.28): every test below that needs a "no key" outcome uses
    /// this instead of an unset env var — `resolve_key`'s keyring fallback
    /// is the real platform store, and a test must never reach it.
    fn no_key(_: &str, _: &str) -> Result<String, cox_protocol::errors::ProviderError> {
        Err(cox_protocol::errors::ProviderError::Auth)
    }

    #[test]
    fn provider_for_custom_builds_chat_client_without_a_key() {
        let p = provider_for_with(&deepseek_config("chat"), no_key).expect("builds");
        assert_eq!(p.id(), ProviderId::Local);
        assert_eq!(p.capabilities().max_context, 1_000_000);
    }

    #[test]
    fn provider_for_custom_responses_builds_responses_client() {
        let p = provider_for_with(&deepseek_config("responses"), no_key).expect("builds");
        assert_eq!(p.id(), ProviderId::OpenAi);
    }

    #[test]
    fn provider_for_rejects_unknown_names_and_shapes() {
        let mut bad = Config::default();
        bad.tiers.code.provider = "weird".into();
        assert!(provider_for(&bad).is_err(), "unknown name bails");
        assert!(
            provider_for_with(&deepseek_config("smoke-signals"), no_key).is_err(),
            "unknown api bails at startup, not mid-turn"
        );
    }

    /// T30.23: `backend_for`'s one lookup still builds the right provider
    /// kind for every section, native and compatible alike (the deepseek
    /// cases above cover the "custom section" leg of the same claim). A49
    /// (T30.28): a fake resolver, not an env var, keeps every branch off
    /// the real keyring.
    #[test]
    fn backend_for_builds_the_right_provider_kind_per_section() {
        fn fake_key(_: &str, _: &str) -> Result<String, cox_protocol::errors::ProviderError> {
            Ok("sk-test".to_string())
        }

        let anthropic = Config::default();
        let p =
            provider_for_with(&anthropic, fake_key).expect("anthropic builds with a resolved key");
        assert_eq!(p.id(), ProviderId::Anthropic);

        let mut openai = Config::default();
        openai.tiers.code.provider = "openai".into();
        let p = provider_for_with(&openai, fake_key).expect("openai builds through openai_shaped");
        assert_eq!(p.id(), ProviderId::OpenAi);

        let mut local = Config::default();
        local.tiers.code.provider = "local".into();
        let p = provider_for_with(&local, fake_key)
            .expect("local goes through the same openai_shaped path as any compatible section");
        assert_eq!(p.id(), ProviderId::Local);
    }

    /// T30.15: `--provider lmstudio` builds through the Anthropic wire
    /// client (R§4.3.2), keyed or keyless alike — the "no key" leg is what
    /// `openai_shaped` already proves for the other families; this proves
    /// the same `.ok()`-turns-missing-into-None shape holds for the
    /// Anthropic-wire arm too. `AnthropicProvider::id()` always reports
    /// `ProviderId::Anthropic` regardless of section (same "wire family,
    /// not vendor" bucketing `local`/compatible sections already use for
    /// `ProviderId::Local` — the model string disambiguates the ledger row).
    #[test]
    fn backend_for_lmstudio_builds_keyed_and_keyless() {
        let mut cfg = Config::default();
        cfg.tiers.code.provider = "lmstudio".into();
        // A model id no built-in catalog row lists, unlike the Anthropic
        // default `Config::default()` otherwise carries — realistic LM
        // Studio usage (`--tier code=<local model>`), and it exercises the
        // literal floor below rather than an accidental catalog hit.
        cfg.tiers.code.model = "prism-ml/bonsai-27b".into();

        let p = provider_for_with(&cfg, no_key).expect("builds keyless (no auth header)");
        assert_eq!(p.id(), ProviderId::Anthropic);

        fn fake_key(_: &str, _: &str) -> Result<String, cox_protocol::errors::ProviderError> {
            Ok("lm-test-token".to_string())
        }
        let p = provider_for_with(&cfg, fake_key).expect("builds with a resolved key");
        assert_eq!(p.id(), ProviderId::Anthropic);
        assert_eq!(
            p.capabilities().max_context,
            32_768,
            "the literal floor when context_window is 0, no server report was given and the catalog has no row for the configured model"
        );
    }

    /// T30.16: the context LM Studio reports as loaded is the session's
    /// window (what compaction fits), overriding the catalog and the floor;
    /// a configured `context_window` still wins over the server.
    #[test]
    fn lmstudio_window_follows_the_served_loaded_context() {
        let mut cfg = Config::default();
        cfg.tiers.code.provider = "lmstudio".into();
        cfg.tiers.code.model = "prism-ml/bonsai-27b".into();
        let raw = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/lmstudio/models.json"),
        )
        .expect("fixture");
        let list: cox_provider::lmstudio::ModelList =
            serde_json::from_str(&raw).expect("fixture parses");
        let served = list.find("prism-ml/bonsai-27b").expect("listed");

        let p = provider_for_served(&cfg, no_key, Some(served), &[]).expect("builds");
        assert_eq!(p.capabilities().max_context, 251_648);

        cfg.providers.lmstudio.context_window = 65_536;
        let p = provider_for_served(&cfg, no_key, Some(served), &[]).expect("builds");
        assert_eq!(p.capabilities().max_context, 65_536);
    }

    /// T30.25 check: a model configured with a 1M context window is
    /// reported as such, not the pre-T30.25 200k literal — the catalog
    /// (built from `config`, T30.24) is consulted for `tiers.code.model`.
    #[test]
    fn anthropic_capabilities_report_the_configured_models_context_window() {
        fn fake_key(_: &str, _: &str) -> Result<String, cox_protocol::errors::ProviderError> {
            Ok("sk-test".to_string())
        }
        let mut cfg = Config::default();
        cfg.tiers.code.model = "claude-big-1m".into();
        cfg.providers.anthropic.models = vec![cox_protocol::config::ProviderModel {
            id: "claude-big-1m".into(),
            context_window: 1_000_000,
            efforts: vec![],
            ..Default::default()
        }];
        let p = provider_for_with(&cfg, fake_key).expect("anthropic builds");
        assert_eq!(p.capabilities().max_context, 1_000_000);
    }

    /// An unconfigured/unknown model still gets the pre-T30.25 200k floor —
    /// the catalog lookup is additive, not a behaviour change when nothing
    /// overrides the model.
    #[test]
    fn anthropic_capabilities_fall_back_to_200k_for_an_unknown_model() {
        fn fake_key(_: &str, _: &str) -> Result<String, cox_protocol::errors::ProviderError> {
            Ok("sk-test".to_string())
        }
        let mut cfg = Config::default();
        cfg.tiers.code.model = "claude-totally-unlisted".into();
        let p = provider_for_with(&cfg, fake_key).expect("anthropic builds");
        assert_eq!(p.capabilities().max_context, 200_000);
    }

    /// The same fixture `cox-provider-openai`'s own Chat tests read
    /// (`crates/cox-provider-openai/src/chat.rs`), one directory further up
    /// from this crate.
    #[cfg(feature = "plugins")]
    fn openai_chat_fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/openai-chat")
            .join(format!("{name}.sse"));
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading fixture {path:?}: {e}"))
    }

    #[cfg(feature = "plugins")]
    fn provider_test_request(model: &str) -> cox_protocol::types::Request {
        use cox_protocol::types::{
            Content, Effort, Message, ModelId, Role, SystemBlock, Thinking, Tier,
        };
        cox_protocol::types::Request {
            tier: Tier::Code,
            job: Job::Main,
            model: ModelId(model.into()),
            system: vec![SystemBlock {
                text: "You are cox.".into(),
                cache: true,
            }],
            tools: vec![],
            messages: vec![Message {
                role: Role::User,
                content: vec![Content::Text {
                    text: "hello".into(),
                }],
            }],
            effort: Effort::High,
            max_tokens: 1024,
            thinking: Thinking::Off,
            cache_breakpoints: vec![],
            stop_sequences: vec![],
        }
    }

    /// T33.17 Check: a granted plugin's `api = "chat"` `[[provider]]` row,
    /// merged by `cox_plugin::provider::merge`, becomes a real
    /// `OpenAiChatProvider` once `tiers.code.provider` names it — same
    /// `providers.custom` "Type-2" arm `deepseek_config`'s tests exercise
    /// above, now fed a plugin section instead of a hand-written one, and
    /// proven against a mock server rather than just its shape (§7a: "cox
    /// drives it with its own wire clients").
    #[cfg(feature = "plugins")]
    #[tokio::test]
    async fn plugin_chat_section_builds_openai_shaped_client() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(openai_chat_fixture("text_only"), "text/event-stream"),
            )
            .mount(&server)
            .await;

        let decl = cox_plugin_api::ProviderDecl {
            name: "acme".into(),
            api: cox_plugin_api::ProviderApi::Chat,
            base_url: server.uri(),
            api_key_env: None,
            auth: cox_plugin_api::ProviderAuth::Bearer,
        };
        let decls = [decl];
        let plugins = [cox_plugin::provider::PluginProviders {
            plugin: "acme-pkg",
            decls: &decls,
        }];
        let merged = cox_plugin::provider::merge(
            &cox_protocol::config::ProvidersConfig::default(),
            &plugins,
        );
        assert!(merged.warnings.is_empty(), "{:?}", merged.warnings);

        let mut config = Config::default();
        config.providers.custom.extend(merged.custom);
        config.tiers.code.provider = "acme".into();
        config.tiers.code.model = "acme-coder".into();

        let provider = provider_for_with(&config, no_key).expect("builds the merged section");
        assert_eq!(provider.id(), ProviderId::Local);

        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        provider
            .stream(
                provider_test_request("acme-coder"),
                tx,
                tokio_util::sync::CancellationToken::new(),
            )
            .await
            .expect("the merged section speaks the wire it was built for");
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        assert!(
            events.iter().any(|e| matches!(
                e,
                cox_protocol::types::ProviderEvent::Stop {
                    stop: cox_protocol::types::StopReason::EndTurn
                }
            )),
            "{events:?}"
        );
    }

    /// T33.17 Check (invariant 8): a full turn against a plugin's merged
    /// `[[provider]]` section writes exactly one ledger row, the same as
    /// any other provider — `Priced` (wrapped in by `provider_for_with`)
    /// prices the call and `cox-core` records it (`session.rs:1340`).
    #[cfg(feature = "plugins")]
    #[tokio::test]
    async fn plugin_provider_request_has_usage_row() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(openai_chat_fixture("text_only"), "text/event-stream"),
            )
            .mount(&server)
            .await;

        let decl = cox_plugin_api::ProviderDecl {
            name: "acme".into(),
            api: cox_plugin_api::ProviderApi::Chat,
            base_url: server.uri(),
            api_key_env: None,
            auth: cox_plugin_api::ProviderAuth::Bearer,
        };
        let decls = [decl];
        let plugins = [cox_plugin::provider::PluginProviders {
            plugin: "acme-pkg",
            decls: &decls,
        }];
        let merged = cox_plugin::provider::merge(
            &cox_protocol::config::ProvidersConfig::default(),
            &plugins,
        );
        let mut config = Config::default();
        config.providers.custom.extend(merged.custom);
        config.tiers.code.provider = "acme".into();
        config.tiers.code.model = "acme-coder".into();

        let provider: Arc<dyn Provider> =
            provider_for_with(&config, no_key).expect("builds the merged section");
        let home = tempfile::tempdir().expect("home");
        let work = tempfile::tempdir().expect("work");
        let store = Arc::new(Store::open(home.path()).expect("store"));
        let session = Session::new(
            config,
            provider,
            vec![],
            store.clone(),
            store.clone(),
            work.path().to_path_buf(),
        )
        .expect("session");
        user_turn(&session, "hello").await;

        let rows = store.usage_for_session(&session.id()).expect("usage query");
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].provider, ProviderId::Local);
    }
}
