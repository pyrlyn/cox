// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Cassette bookkeeping for the `Replay` provider (T32.11): the sha256
//! hash key derived from a `Request` with volatile fields masked, secret
//! redaction so a cassette can be committed, and the file-write/near-miss
//! helpers `cox record` and `Replay` both use. Depends only on
//! `cox-protocol`, `serde_json` and `sha2` — no HTTP, no SSE parsing. The
//! `Replay` provider that reads a cassette's SSE body back and feeds it
//! through `AnthropicStream` stays in `cox-provider::replay`, because that
//! state machine has not split into its own crate yet (T32.13).

use std::path::Path;

use cox_protocol::errors::ProviderError;
use cox_protocol::types::Request;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Replaces `sk-` keys and `Bearer ` prefixes so a cassette can be committed.
pub fn redact_secrets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(stripped) = rest.strip_prefix("Bearer ") {
            out.push_str("«redacted»");
            let end = stripped
                .find(|c: char| c.is_ascii_whitespace())
                .unwrap_or(stripped.len());
            rest = &stripped[end..];
            continue;
        }
        if let Some(stripped) = rest.strip_prefix("sk-") {
            // Anthropic (`sk-ant-api03-…`) and OpenAI project keys carry `-`
            // and `_` in the body, as in `cox_sanitize::redact::scrub` (T50.7);
            // that crate is off-limits here (`deps.rs` keeps this a leaf).
            let n = stripped
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
                .count();
            if n >= 8 {
                out.push_str("«redacted»");
                rest = &stripped[n..];
                continue;
            }
        }
        let Some(ch) = rest.chars().next() else {
            break;
        };
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    out
}

/// Canonical cassette key for `req`: volatile fields masked, then sha256.
pub fn cassette_hash(req: &Request) -> Result<String, ProviderError> {
    let mut value = serde_json::to_value(req).map_err(|e| ProviderError::BadRequest {
        message: format!("canonical request: {e}"),
    })?;
    mask_volatile(&mut value);
    let bytes = serde_json::to_vec(&value).map_err(|e| ProviderError::BadRequest {
        message: format!("canonical request: {e}"),
    })?;
    Ok(hex_sha256(&bytes))
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn mask_volatile(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if matches!(k.as_str(), "date" | "cwd" | "created_at") {
                    *v = Value::String(String::new());
                } else {
                    mask_volatile(v);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(mask_volatile),
        _ => {}
    }
}

/// Writes `<hash>.request.json` and `<hash>.sse` under `dir`.
pub fn write_cassette(
    dir: &Path,
    req: &Request,
    sse: &str,
    redact: bool,
) -> Result<String, ProviderError> {
    std::fs::create_dir_all(dir).map_err(|_| ProviderError::Network)?;
    let hash = cassette_hash(req)?;
    let req_json = serde_json::to_string_pretty(req).map_err(|e| ProviderError::BadRequest {
        message: e.to_string(),
    })?;
    let sse = if redact {
        redact_secrets(sse)
    } else {
        sse.to_string()
    };
    let req_json = if redact {
        redact_secrets(&req_json)
    } else {
        req_json
    };
    std::fs::write(dir.join(format!("{hash}.request.json")), req_json)
        .map_err(|_| ProviderError::Network)?;
    std::fs::write(dir.join(format!("{hash}.sse")), sse).map_err(|_| ProviderError::Network)?;
    Ok(hash)
}

/// The nearest cassette stem to `hash` under `dir`, for a miss's error
/// message — the run's actual request usually differs from the recorded
/// one by a single field, so the longest shared hex prefix is almost
/// always the intended cassette.
pub fn nearest_hint(dir: &Path, hash: &str) -> String {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return String::new();
    };
    let mut best: Option<(usize, String)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".sse") else {
            continue;
        };
        let shared = stem
            .chars()
            .zip(hash.chars())
            .take_while(|(a, b)| a == b)
            .count();
        if best.as_ref().is_none_or(|(n, _)| shared > *n) {
            best = Some((shared, stem.to_string()));
        }
    }
    match best {
        Some((_, stem)) => format!("\nnearest: {stem}"),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cox_protocol::types::{Effort, Job, ModelId, Thinking, Tier};

    fn req() -> Request {
        Request {
            tier: Tier::Code,
            job: Job::Main,
            model: ModelId("claude-sonnet-5".into()),
            system: vec![],
            tools: vec![],
            messages: vec![],
            effort: Effort::High,
            max_tokens: 1024,
            thinking: Thinking::Off,
            cache_breakpoints: vec![],
            stop_sequences: vec![],
        }
    }

    #[test]
    fn redact_strips_sk_and_bearer() {
        let raw = "key=sk-abcdefghijk Authorization: Bearer tokensecret\n";
        let redacted = redact_secrets(raw);
        assert!(!redacted.contains("sk-abcdefghijk"));
        assert!(!redacted.contains("Bearer "));
        assert!(redacted.contains("«redacted»"));
    }

    #[test]
    fn cassette_redaction_removes_an_anthropic_key_whole() {
        let line = "x-api-key: sk-ant-api03-R2D2_c3po-XyZ0123456789-abcDEF_ghiAA end";
        assert_eq!(redact_secrets(line), "x-api-key: «redacted» end");
    }

    #[test]
    fn redact_preserves_non_ascii() {
        let raw = "café sk-abcdefghijk 日本語";
        let redacted = redact_secrets(raw);
        assert!(redacted.contains("café"));
        assert!(redacted.contains("日本語"));
        assert!(!redacted.contains("sk-abcdefghijk"));
    }

    #[test]
    fn cassette_hash_is_stable_for_same_request() {
        assert_eq!(
            cassette_hash(&req()).expect("hash"),
            cassette_hash(&req()).expect("hash")
        );
    }

    #[test]
    fn nearest_hint_picks_longest_shared_prefix() {
        let dir = std::env::temp_dir().join(format!(
            "cox-testkit-nearest-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create dir");
        std::fs::write(dir.join("abcd1111.sse"), "").expect("write");
        std::fs::write(dir.join("abcx2222.sse"), "").expect("write");
        let hint = nearest_hint(&dir, "abcd9999");
        assert_eq!(hint, "\nnearest: abcd1111");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
