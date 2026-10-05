// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! LM Studio's native REST API (`/api/v1`, T30.16): the subset cox reads to
//! learn what the server is actually running — the loaded context length,
//! whether the model was trained for tool use, whether it is loaded at all —
//! and `models/load` to load it on demand. Chat itself stays on the
//! Anthropic-compatible `/v1/messages` (T30.15), because `/api/v1/chat`
//! takes no custom tool schemas (R§4.3.2); this module never streams.
//!
//! The wire types are hand-written (D3/A40 step 3): LM Studio ships no Rust
//! SDK and no published spec. Unknown fields are ignored, so a newer server
//! adding fields does not break a session. The shape was checked against a
//! live server (`fixtures/lmstudio/models.json`) and, for `models/load`,
//! against https://lmstudio.ai/docs/developer/rest/load.

use cox_protocol::config::Transport;
use cox_protocol::errors::ProviderError;
use serde::{Deserialize, Serialize};

/// `GET /api/v1/models`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ModelList {
    /// Every model the server has downloaded, loaded or not.
    #[serde(default)]
    pub models: Vec<Model>,
}

impl ModelList {
    /// The entry whose `key` is `model` — the id `/v1/messages` takes.
    pub fn find(&self, model: &str) -> Option<&Model> {
        self.models.iter().find(|m| m.key == model)
    }
}

/// One downloaded model.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Model {
    /// The model id, e.g. `prism-ml/bonsai-27b`.
    pub key: String,
    /// The most context the model supports.
    #[serde(default)]
    pub max_context_length: Option<u32>,
    /// Running instances; empty when the model is not loaded.
    #[serde(default)]
    pub loaded_instances: Vec<LoadedInstance>,
    /// Absent on embedding models.
    #[serde(default)]
    pub capabilities: Option<Capabilities>,
}

impl Model {
    /// The context window compaction must fit: the smallest loaded
    /// instance's, since any of them may serve a request. `None` when the
    /// model is not loaded.
    pub fn loaded_context(&self) -> Option<u32> {
        self.loaded_instances
            .iter()
            .map(|i| i.config.context_length)
            .min()
    }

    /// `capabilities.trained_for_tool_use`, when the server reports it.
    pub fn tool_use(&self) -> Option<bool> {
        self.capabilities.as_ref()?.trained_for_tool_use
    }

    /// One line per thing a session should warn about: a model that was not
    /// trained for tool use drives cox's tool loop poorly, and an unloaded
    /// one leaves the context window to the catalog's guess.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.tool_use() != Some(true) {
            out.push(format!(
                "LM Studio reports `{}` as not trained for tool use; tool calls may fail",
                self.key
            ));
        }
        if self.loaded_context().is_none() {
            out.push(format!(
                "`{}` is not loaded in LM Studio, so its context window is a guess; \
                 set providers.lmstudio.load = true to load it at session start",
                self.key
            ));
        }
        out
    }
}

/// `loaded_instances[]`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct LoadedInstance {
    /// The instance id.
    #[serde(default)]
    pub id: String,
    /// What the instance was loaded with.
    #[serde(default)]
    pub config: InstanceConfig,
}

/// `loaded_instances[].config`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct InstanceConfig {
    /// The context length actually allocated — not necessarily the one
    /// asked for (R§4.3.2: `--context-length 65536` left 251 648).
    #[serde(default)]
    pub context_length: u32,
}

/// `capabilities`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Capabilities {
    /// Whether the model was trained for tool use.
    #[serde(default)]
    pub trained_for_tool_use: Option<bool>,
    /// Reasoning toggles, when the model has any.
    #[serde(default)]
    pub reasoning: Option<Reasoning>,
}

/// `capabilities.reasoning`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Reasoning {
    /// E.g. `["off", "on"]`.
    #[serde(default)]
    pub allowed_options: Vec<String>,
}

/// `POST /api/v1/models/load` body. Only the fields cox sets.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LoadRequest {
    /// The model key.
    pub model: String,
    /// Omitted to let the server pick its default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
    /// Asks the server to return `load_config`, the fallback when the
    /// model list does not show the new instance yet.
    pub echo_load_config: bool,
}

/// `POST /api/v1/models/load` response.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct LoadResponse {
    /// The new instance's id.
    #[serde(default)]
    pub instance_id: String,
    /// `"loaded"` on success.
    #[serde(default)]
    pub status: String,
    /// Present when `echo_load_config` was set.
    #[serde(default)]
    pub load_config: Option<InstanceConfig>,
}

/// A client for the native API of one LM Studio server.
#[derive(Debug, Clone)]
pub struct LmStudio {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
}

impl LmStudio {
    /// Builds a client over `[providers.lmstudio]`'s transport. The native
    /// API takes the same token as `/v1/messages`, but as a bearer token.
    pub fn new(transport: &Transport, api_key: Option<String>) -> Result<Self, ProviderError> {
        Ok(Self {
            http: crate::http::client_with_timeout(transport.timeout_s)?,
            base_url: transport.base_url.trim_end_matches('/').to_string(),
            api_key,
        })
    }

    /// `GET /api/v1/models`.
    pub async fn models(&self) -> Result<ModelList, ProviderError> {
        let req = self.http.get(format!("{}/api/v1/models", self.base_url));
        self.send(req).await
    }

    /// `POST /api/v1/models/load`.
    pub async fn load(&self, body: &LoadRequest) -> Result<LoadResponse, ProviderError> {
        let req = self
            .http
            .post(format!("{}/api/v1/models/load", self.base_url))
            .json(body);
        self.send(req).await
    }

    /// What a session needs before it starts: `model`'s entry, loaded first
    /// when it is not and `load` is set (with `context_length`, or the
    /// server's default when `None`). A model the server does not list is
    /// a `BadRequest` — the first chat call would fail anyway.
    pub async fn prepare(
        &self,
        model: &str,
        load: bool,
        context_length: Option<u32>,
    ) -> Result<Model, ProviderError> {
        let entry = self.listed(model).await?;
        if !load || entry.loaded_context().is_some() {
            return Ok(entry);
        }
        let loaded = self
            .load(&LoadRequest {
                model: model.to_string(),
                context_length,
                echo_load_config: true,
            })
            .await?;
        // R§4.3.2: what was allocated is read back, never assumed from the
        // request; the echoed config only fills in when the list lags.
        let mut entry = self.listed(model).await?;
        if entry.loaded_instances.is_empty()
            && let Some(config) = loaded.load_config
        {
            entry.loaded_instances.push(LoadedInstance {
                id: loaded.instance_id,
                config,
            });
        }
        Ok(entry)
    }

    async fn listed(&self, model: &str) -> Result<Model, ProviderError> {
        self.models()
            .await?
            .find(model)
            .cloned()
            .ok_or_else(|| ProviderError::BadRequest {
                message: format!("LM Studio at {} lists no model `{model}`", self.base_url),
            })
    }

    async fn send<T: for<'de> Deserialize<'de>>(
        &self,
        mut req: reqwest::RequestBuilder,
    ) -> Result<T, ProviderError> {
        if let Some(key) = &self.api_key {
            req = req.header("authorization", crate::http::bearer(key)?);
        }
        let response = req.send().await.map_err(|e| {
            if e.is_timeout() {
                ProviderError::Timeout
            } else {
                ProviderError::Network
            }
        })?;
        let status = response.status();
        let text = response.text().await.map_err(|_| ProviderError::Network)?;
        if !status.is_success() {
            return Err(crate::http::map_http_error(status, &text, None));
        }
        serde_json::from_str(&text).map_err(|e| ProviderError::Parse {
            line: e.line() as u64,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const MODEL: &str = "prism-ml/bonsai-27b";

    fn fixture() -> serde_json::Value {
        let raw = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/lmstudio/models.json"),
        )
        .expect("fixture");
        serde_json::from_str(&raw).expect("fixture json")
    }

    /// The live capture with every model unloaded.
    fn unloaded() -> serde_json::Value {
        let mut v = fixture();
        for m in v["models"].as_array_mut().expect("models") {
            m["loaded_instances"] = serde_json::json!([]);
        }
        v
    }

    fn client(server: &MockServer, key: Option<&str>) -> LmStudio {
        let transport = Transport {
            base_url: server.uri(),
            api_key_env: String::new(),
            timeout_s: 5,
            max_retries: 0,
        };
        LmStudio::new(&transport, key.map(str::to_string)).expect("client")
    }

    #[test]
    fn live_fixture_parses_loaded_context_and_capabilities() {
        let list: ModelList = serde_json::from_value(fixture()).expect("parses");
        let m = list.find(MODEL).expect("listed");
        assert_eq!(m.loaded_context(), Some(251_648));
        assert_eq!(m.max_context_length, Some(262_144));
        assert_eq!(m.tool_use(), Some(true));
        let reasoning = m.capabilities.as_ref().and_then(|c| c.reasoning.as_ref());
        assert_eq!(
            reasoning.map(|r| r.allowed_options.clone()),
            Some(vec!["off".to_string(), "on".to_string()])
        );
        assert!(m.warnings().is_empty());
        // The embedding model has no capabilities: not a tool-use model.
        let embed = list
            .find("text-embedding-nomic-embed-text-v1.5")
            .expect("listed");
        assert_eq!(embed.loaded_context(), None);
        assert_eq!(embed.warnings().len(), 2, "no tool use, not loaded");
    }

    #[tokio::test]
    async fn prepare_reads_a_loaded_model_without_loading_it() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/models"))
            .and(header("authorization", "Bearer lm-test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/models/load"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let m = client(&server, Some("lm-test"))
            .prepare(MODEL, true, Some(65_536))
            .await
            .expect("prepared");
        assert_eq!(m.loaded_context(), Some(251_648));
    }

    #[tokio::test]
    async fn prepare_leaves_an_unloaded_model_alone_without_load() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(unloaded()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/models/load"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let m = client(&server, None)
            .prepare(MODEL, false, None)
            .await
            .expect("prepared");
        assert_eq!(m.loaded_context(), None);
    }

    #[tokio::test]
    async fn prepare_loads_with_the_configured_context_and_reads_it_back() {
        let server = MockServer::start().await;
        // First read: not loaded; after the load call: loaded at 65 536.
        Mock::given(method("GET"))
            .and(path("/api/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(unloaded()))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        let mut after = fixture();
        after["models"][0]["loaded_instances"][0]["config"]["context_length"] =
            serde_json::json!(65_536);
        Mock::given(method("GET"))
            .and(path("/api/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(after))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/models/load"))
            .and(body_json(serde_json::json!({
                "model": MODEL,
                "context_length": 65_536,
                "echo_load_config": true,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "type": "llm",
                "instance_id": MODEL,
                "load_time_seconds": 9.1,
                "status": "loaded",
                "load_config": {"context_length": 65_536}
            })))
            .expect(1)
            .mount(&server)
            .await;
        let m = client(&server, None)
            .prepare(MODEL, true, Some(65_536))
            .await
            .expect("prepared");
        assert_eq!(m.loaded_context(), Some(65_536));
    }

    #[tokio::test]
    async fn prepare_rejects_a_model_the_server_does_not_list() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture()))
            .mount(&server)
            .await;
        let err = client(&server, None)
            .prepare("nope/absent", true, None)
            .await
            .expect_err("not listed");
        assert!(matches!(err, ProviderError::BadRequest { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn an_unreachable_server_is_a_network_error() {
        // A port that was free a moment ago: nothing listens there. Not a
        // dropped `MockServer`, which wiremock pools and keeps listening.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .expect("free port")
            .port();
        let transport = Transport {
            base_url: format!("http://127.0.0.1:{port}"),
            api_key_env: String::new(),
            timeout_s: 5,
            max_retries: 0,
        };
        let dead = LmStudio::new(&transport, None).expect("client");
        let err = dead.models().await.expect_err("server is down");
        assert!(matches!(err, ProviderError::Network), "{err:?}");
    }
}
