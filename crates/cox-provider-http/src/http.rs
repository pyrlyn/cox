// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Shared HTTP plumbing for the network backends: credential lookup, auth
//! headers, and non-2xx → [`ProviderError`] mapping (plan.md §1.14).
//!
//! Anthropic and OpenAI-shaped servers phrase errors differently but map to
//! the same taxonomy, so the union lives here once instead of once per
//! backend. Every status a contract test asserts on maps exactly as before;
//! the only deliberate widening is that a 5xx is `Overloaded` (retryable) on
//! every backend — a 500 from Anthropic is transient per their own docs, and
//! treating it as a fatal `BadRequest` retried nothing.

use std::time::Duration;

use cox_protocol::errors::ProviderError;
use reqwest::header::HeaderValue;

/// How long a TCP/TLS handshake may take before a call is `Network`. Shared
/// by every backend that builds its own `reqwest::Client` (T30.23) — used to
/// be duplicated as `anthropic::CONNECT_TIMEOUT`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Builds the connection-pooled client every network backend uses: a
/// bounded connect timeout plus a read/idle timeout from the section's
/// `timeout_s` (`providers.<name>.timeout_s`). The read timeout is the gap
/// between bytes on an open stream, not a whole-call cap, so a long answer
/// keeps streaming for minutes without ever going idle.
pub fn client_with_timeout(timeout_s: u32) -> Result<reqwest::Client, ProviderError> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(Duration::from_secs(u64::from(timeout_s).max(1)))
        .build()
        .map_err(|_| ProviderError::Network)
}

/// `env_var` first, else the platform keyring entry `cox/<section>` (the
/// native store per platform, not a mock). Every provider section resolves
/// its key this way — `docs/design/providers.md` § Target shape, item 2
/// (T30.21): `[providers.anthropic]`, `[providers.openai]`, `[providers.
/// typesafe]` and every `[providers.<name>]` compatible section all read
/// their own `api_key_env` first, then `cox/<section>`.
///
/// A missing or unreadable credential is [`ProviderError::Auth`]; callers
/// decide what that means for their section. Anthropic and Jev always need
/// a key, so they propagate the error with `?`. OpenAI-shaped sections
/// (`openai`, `local`, and any compatible section) build with an
/// `Option<String>` key and call `.ok()`: a local server or a self-hosted
/// gateway commonly needs none, so a missing key there is "no
/// `Authorization` header", not a failure. Called on every provider
/// construction, including in `cox doctor`, so it never panics.
pub fn resolve_key(env_var: &str, section: &str) -> Result<String, ProviderError> {
    resolve_key_with(env_var, section, platform_keyring)
}

/// [`resolve_key`]'s body, taking the keyring lookup as a parameter so
/// tests can exercise the env-then-keyring precedence without touching the
/// real platform store: `keyring::Entry`'s v1-compatibility shim binds to
/// the real Keychain/Credential-Manager/Secret-Service the first time
/// *anything* touches it, once per process and irreversibly (it is a
/// `LazyLock` that always wins over a prior `set_default_store`), so no
/// mock swapped in afterwards would ever be consulted. `pub` (T32.12: this
/// crate is a dependency of `cox-provider`, not a module inside it) so
/// every wire's own tests, wherever they live, can inject a fake lookup too
/// (A49, T30.28) instead of calling [`resolve_key`] and reaching the real
/// keyring.
pub fn resolve_key_with(
    env_var: &str,
    section: &str,
    keyring_lookup: impl FnOnce(&str) -> Option<String>,
) -> Result<String, ProviderError> {
    match std::env::var(env_var) {
        Ok(k) if !k.trim().is_empty() => return Ok(k),
        _ => {}
    }
    keyring_lookup(section).ok_or(ProviderError::Auth)
}

/// The real platform keyring entry `cox/<section>`, or nothing when
/// `COX_KEYRING=off` (A49): cargo sets that for every test and dev run so a
/// rebuilt binary never raises a keychain prompt.
fn platform_keyring(section: &str) -> Option<String> {
    let switch = std::env::var(cox_protocol::config::KEYRING_ENV).ok();
    if !cox_protocol::config::keyring_enabled(switch.as_deref()) {
        return None;
    }
    keyring::Entry::new("cox", section)
        .and_then(|e| e.get_password())
        .ok()
}

/// `Authorization: Bearer <key>`, marked sensitive. A key with non-ASCII
/// bytes is a misconfigured credential (`Auth`), not a transport failure.
pub fn bearer(key: &str) -> Result<HeaderValue, ProviderError> {
    let mut v = HeaderValue::from_str(&format!("Bearer {key}")).map_err(|_| ProviderError::Auth)?;
    v.set_sensitive(true);
    Ok(v)
}

/// `x-api-key: <key>`, marked sensitive. Same Auth-on-garbage rule as
/// [`bearer`].
pub fn api_key(key: &str) -> Result<HeaderValue, ProviderError> {
    let mut v = HeaderValue::from_str(key).map_err(|_| ProviderError::Auth)?;
    v.set_sensitive(true);
    Ok(v)
}

/// The `{"error": {"message": ...}}` envelope both API families use (Ollama
/// included); falls back to the raw body so a proxy's plain-text error is
/// not lost.
pub fn error_message(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error")?.get("message")?.as_str().map(str::to_string))
        .unwrap_or_else(|| body.to_string())
}

/// Best-effort `(got, max)` extraction from a too-long/context-exceeded
/// message. Union of both families' phrasings ("too long", "exceed" —
/// Ollama says "exceeds context length", Anthropic says "too long"); no
/// documented fixed schema exists, so anything else falls back to
/// `BadRequest` at the call site rather than guessing.
pub fn parse_context_too_long(message: &str) -> Option<(u32, u32)> {
    let lower = message.to_ascii_lowercase();
    if !(lower.contains("too long") || lower.contains("exceed")) {
        return None;
    }
    let mut numbers = message
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<u32>().ok());
    let got = numbers.next()?;
    let max = numbers.next()?;
    Some((got, max))
}

/// Maps a non-2xx response to a [`ProviderError`]. `retry_after` comes from
/// the `retry-after` header, read before the body is consumed.
pub fn map_http_error(
    status: reqwest::StatusCode,
    body: &str,
    retry_after: Option<u64>,
) -> ProviderError {
    let message = error_message(body);
    match status.as_u16() {
        401 | 403 => ProviderError::Auth,
        429 => ProviderError::RateLimited { retry_after },
        500 | 502 | 503 | 504 | 529 => ProviderError::Overloaded,
        // A too-long prompt is a 400 `invalid_request_error` in practice
        // (413 is reserved for raw request-body size); handled the same way
        // regardless of which status carried it.
        400 | 413 => match parse_context_too_long(&message) {
            Some((got, max)) => ProviderError::ContextTooLong { max, got },
            None => ProviderError::BadRequest { message },
        },
        _ => ProviderError::BadRequest { message },
    }
}

#[cfg(test)]
// why: env::set_var/remove_var are unsafe in edition 2024 (per-process tests).
#[allow(unsafe_code)]
mod tests {
    use super::*;

    #[test]
    fn http_union_mapping_preserves_both_backends_cases() {
        // Anthropic's asserted cases.
        assert!(matches!(
            map_http_error(reqwest::StatusCode::UNAUTHORIZED, "{}", None),
            ProviderError::Auth
        ));
        assert!(matches!(
            map_http_error(reqwest::StatusCode::TOO_MANY_REQUESTS, "{}", Some(30)),
            ProviderError::RateLimited {
                retry_after: Some(30)
            }
        ));
        assert!(matches!(
            map_http_error(reqwest::StatusCode::SERVICE_UNAVAILABLE, "{}", None),
            ProviderError::Overloaded
        ));
        let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 250000 tokens > 200000 maximum"}}"#;
        assert_eq!(
            map_http_error(reqwest::StatusCode::BAD_REQUEST, body, None),
            ProviderError::ContextTooLong {
                max: 200_000,
                got: 250_000,
            }
        );
        // Chat's asserted cases: 403 is auth, Ollama's "exceeds" phrasing parses.
        assert!(matches!(
            map_http_error(reqwest::StatusCode::FORBIDDEN, "{}", None),
            ProviderError::Auth
        ));
        let body = r#"{"error":{"message":"input length exceeds context length: 40000 tokens > 32768 maximum","type":"invalid_request_error"}}"#;
        assert_eq!(
            map_http_error(reqwest::StatusCode::BAD_REQUEST, body, None),
            ProviderError::ContextTooLong {
                max: 32_768,
                got: 40_000,
            }
        );
        // Plain-text proxy body survives.
        assert_eq!(
            map_http_error(reqwest::StatusCode::BAD_REQUEST, "nope", None),
            ProviderError::BadRequest {
                message: "nope".into()
            }
        );
    }

    #[test]
    fn http_bearer_and_api_key_reject_garbage_as_auth() {
        assert!(bearer("sk-test").is_ok());
        assert!(api_key("sk-test").is_ok());
        // A control byte can never be a credential: auth problem, not transport.
        assert!(matches!(bearer("a\nb"), Err(ProviderError::Auth)));
        assert!(matches!(api_key("a\nb"), Err(ProviderError::Auth)));
    }

    /// A49: `.cargo/config.toml` switches the keyring off for every test
    /// process, so even a lookup that slips past the injected seams finds
    /// nothing instead of raising a keychain prompt.
    #[test]
    fn tests_run_with_the_keyring_switched_off() {
        let switch = std::env::var(cox_protocol::config::KEYRING_ENV).ok();
        assert!(!cox_protocol::config::keyring_enabled(switch.as_deref()));
    }

    #[test]
    fn resolve_key_prefers_the_env_var_over_the_keyring() {
        // Safety: cargo nextest runs each #[test] as its own process, so
        // mutating this env var here cannot race another test's read of it.
        unsafe { std::env::set_var("COX_TEST_RESOLVE_KEY_ENV_WINS", "sk-env") };
        let got = resolve_key_with("COX_TEST_RESOLVE_KEY_ENV_WINS", "test-section", |_| {
            panic!("the keyring must not be consulted when the env var is set")
        });
        assert_eq!(got.ok().as_deref(), Some("sk-env"));
        unsafe { std::env::remove_var("COX_TEST_RESOLVE_KEY_ENV_WINS") };
    }

    /// A compatible section (`[providers.<name>]`) with no env var set
    /// falls back to the keyring entry `cox/<name>` — the same rule every
    /// other section follows through this one resolver.
    #[test]
    fn resolve_key_falls_back_to_the_keyring_when_the_env_var_is_unset() {
        // Safety: see above.
        unsafe { std::env::remove_var("COX_TEST_RESOLVE_KEY_ENV_MISSING") };
        let got = resolve_key_with(
            "COX_TEST_RESOLVE_KEY_ENV_MISSING",
            "test-section",
            |section| {
                assert_eq!(section, "test-section");
                Some("sk-from-keyring".to_string())
            },
        );
        assert_eq!(got.ok().as_deref(), Some("sk-from-keyring"));
    }

    #[test]
    fn resolve_key_is_auth_error_when_neither_env_nor_keyring_has_one() {
        // Safety: see above.
        unsafe { std::env::remove_var("COX_TEST_RESOLVE_KEY_ENV_ABSENT") };
        let got = resolve_key_with("COX_TEST_RESOLVE_KEY_ENV_ABSENT", "test-section", |_| None);
        assert!(matches!(got, Err(ProviderError::Auth)));
    }
}
