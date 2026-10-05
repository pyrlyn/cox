// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox_http` (PL§4, PL§7d, T33.14.1): which URLs a plugin may reach and
//! the one request path to them. Separate from `hostfn` because that module
//! decides what an import may do in general, while this one owns the network
//! rule, which T33.40.1 extends with provider hosts reachable only from
//! `cox_provider_stream`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;
use std::time::Duration;

use cox_plugin_api::{AbiError, Capabilities, HttpReq, HttpResp, PluginManifest};
use cox_protocol::config::ProvidersConfig;
use reqwest::header::{HOST, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, Url, redirect};

/// `max_http_response_bytes` (PL§4): a larger body is refused, not cut, so
/// a plugin never mistakes a truncated reply for the whole one.
pub const MAX_HTTP_RESPONSE_BYTES: usize = 1 << 20;
/// A host function cannot be cancelled by the call's extism deadline, so
/// the request carries its own.
const TIMEOUT: Duration = Duration::from_secs(30);

/// One plugin's network rule: its granted `net:<host>` lines.
pub(crate) struct Net {
    allow: Capabilities,
    client: OnceLock<Result<reqwest::Client, String>>,
}

impl Net {
    /// From the granted capability lines (`grant::capability_list`).
    pub(crate) fn new(granted: &BTreeSet<String>) -> Self {
        let net = granted
            .iter()
            .filter_map(|line| line.strip_prefix("net:"))
            .map(str::to_owned)
            .collect();
        Self {
            allow: Capabilities {
                net,
                ..Capabilities::default()
            },
            client: OnceLock::new(),
        }
    }

    /// Where `url` may go. The host is matched with `net_allows`, the one
    /// matcher every `net` check uses. T33.40.1 adds its provider-host
    /// branch here, ahead of the `net` check, keyed on the calling export.
    pub(crate) fn target(&self, url: &str) -> Result<Url, AbiError> {
        if self.allow.net.is_empty() {
            return Err(AbiError::NotGranted {
                capability: "net".into(),
            });
        }
        let url = Url::parse(url).map_err(|e| failed(&format!("bad url: {e}")))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(failed("only http and https urls"));
        }
        let host = url
            .host_str()
            .ok_or_else(|| failed("the url has no host"))?;
        if !self.allow.net_allows(host) {
            return Err(AbiError::NotGranted {
                capability: format!("net:{host}"),
            });
        }
        Ok(url)
    }

    /// Sends `request` to `url` (already through [`Net::target`]).
    pub(crate) async fn send(&self, request: HttpReq, url: Url) -> Result<HttpResp, AbiError> {
        let method =
            Method::from_bytes(request.method.as_bytes()).map_err(|_| failed("bad http method"))?;
        let mut headers = HeaderMap::new();
        for (name, value) in &request.headers {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| failed(&format!("bad header name {name:?}")))?;
            // The allow-list checks the url's host; a Host header naming
            // another site on the same server would get around it.
            if name == HOST {
                return Err(failed("the Host header comes from the url"));
            }
            let value = HeaderValue::from_str(value).map_err(|_| failed("bad header value"))?;
            headers.append(name, value);
        }
        let client = self
            .client
            .get_or_init(client)
            .as_ref()
            .map_err(|e| failed(e))?;
        let mut builder = client.request(method, url).headers(headers);
        if let Some(body) = request.body {
            builder = builder.body(body);
        }
        let mut response = builder.send().await.map_err(transport)?;
        let too_large = AbiError::TooLarge {
            limit: MAX_HTTP_RESPONSE_BYTES as u64,
        };
        if response
            .content_length()
            .is_some_and(|n| n > MAX_HTTP_RESPONSE_BYTES as u64)
        {
            return Err(too_large);
        }
        let status = response.status().as_u16();
        let mut joined: BTreeMap<String, String> = BTreeMap::new();
        for (name, value) in response.headers() {
            let value = String::from_utf8_lossy(value.as_bytes());
            joined
                .entry(name.as_str().to_owned())
                .and_modify(|v| {
                    v.push_str(", ");
                    v.push_str(&value);
                })
                .or_insert_with(|| value.into_owned());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(transport)? {
            if body.len() + chunk.len() > MAX_HTTP_RESPONSE_BYTES {
                return Err(too_large);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(HttpResp {
            status,
            headers: joined,
            body: String::from_utf8_lossy(&body).into_owned(),
        })
    }
}

/// Redirects are off: a 3xx to a host outside the allow-list would
/// otherwise be followed without a check. The guest sees the 3xx and may
/// ask again, through [`Net::target`].
fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(redirect::Policy::none())
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| format!("http client: {e}"))
}

fn transport(e: reqwest::Error) -> AbiError {
    if e.is_timeout() {
        AbiError::Timeout
    } else if e.is_connect() {
        AbiError::Transient {
            message: e.to_string(),
        }
    } else {
        failed(&e.to_string())
    }
}

fn failed(message: &str) -> AbiError {
    AbiError::Failed {
        message: message.into(),
    }
}

/// A `net` entry that covers a provider host (PL§7d).
#[derive(Debug, thiserror::Error)]
#[error(
    "its net allow-list covers the provider host {0}; a provider is reached through a [[provider]] section or cox_model_call"
)]
pub struct ProviderHostInNet(pub String);

/// PL§7d: raw `cox_http` to a paid API would skip the ledger and the budget
/// and need the key inside the guest, so a manifest whose `net` covers the
/// host of a configured provider section, or of its own `[[provider]]`
/// rows, is refused before it loads.
pub fn refuse_provider_hosts(
    manifest: &PluginManifest,
    providers: &ProvidersConfig,
) -> Result<(), ProviderHostInNet> {
    let native = [
        &providers.anthropic.base_url,
        &providers.openai.base_url,
        &providers.local.base_url,
        &providers.lmstudio.base_url,
        &providers.typesafe.base_url,
    ];
    let hosts = native
        .into_iter()
        .chain(providers.custom.values().map(|c| &c.base_url))
        .chain(manifest.provider.iter().map(|p| &p.base_url))
        .filter_map(|base| Url::parse(base).ok()?.host_str().map(str::to_owned));
    for host in hosts {
        if manifest.capabilities.net_allows(&host) {
            return Err(ProviderHostInNet(host));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use cox_protocol::config::CompatibleProviderConfig;
    use serde_json::json;

    use super::*;

    fn manifest(net: &[&str], provider: serde_json::Value) -> PluginManifest {
        serde_json::from_value(json!({
            "api": 1, "id": "p", "version": "0.1.0", "name": "p", "wasm": "plugin.wasm",
            "capabilities": { "net": net }, "provider": provider,
        }))
        .expect("manifest")
    }

    #[test]
    fn net_entry_matching_provider_host_is_rejected() {
        let mut providers = ProvidersConfig::default();
        providers.custom.insert(
            "deepseek".into(),
            CompatibleProviderConfig {
                base_url: "https://api.deepseek.com/v1".into(),
                ..CompatibleProviderConfig::default()
            },
        );
        let refused = |net: &[&str], provider| {
            refuse_provider_hosts(&manifest(net, provider), &providers)
                .err()
                .map(|e| e.0)
        };
        assert_eq!(
            refused(&["api.anthropic.com"], json!([])).as_deref(),
            Some("api.anthropic.com")
        );
        assert_eq!(
            refused(&["*.deepseek.com"], json!([])).as_deref(),
            Some("api.deepseek.com")
        );
        let own =
            json!([{ "name": "own", "api": "plugin", "base_url": "https://llm.example.com" }]);
        assert_eq!(
            refused(&["llm.example.com"], own.clone()).as_deref(),
            Some("llm.example.com")
        );
        assert_eq!(refused(&["api.github.com"], own), None);
    }

    #[test]
    fn target_matches_the_url_host_not_its_userinfo() {
        let granted = BTreeSet::from(["net:api.github.com".to_owned()]);
        let net = Net::new(&granted);
        assert!(net.target("https://api.github.com/repos").is_ok());
        assert_eq!(
            net.target("https://api.github.com@evil.example/").err(),
            Some(AbiError::NotGranted {
                capability: "net:evil.example".into()
            })
        );
        assert!(matches!(
            net.target("file:///etc/passwd"),
            Err(AbiError::Failed { .. })
        ));
        assert_eq!(
            Net::new(&BTreeSet::new())
                .target("https://api.github.com/")
                .err(),
            Some(AbiError::NotGranted {
                capability: "net".into()
            })
        );
    }
}
