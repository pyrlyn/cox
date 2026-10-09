// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The contract a model is allowed to see for one MCP tool (T64.24).
//! A description and a `readOnlyHint` are written by the server, so they
//! stay out of the prompt and out of the approval bypass until the stored
//! hash of `name|description|canonical schema` matches. Annotations are
//! not hashed: a hint must not be able to change the contract, and it must
//! not lower risk while the contract is untrusted.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use cox_protocol::StoreError;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Whether the model may see this tool's real definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolTrust {
    /// The stored hash is the hash of the definition being listed.
    Approved,
    /// No approved hash yet, and the server did not come from the user.
    Pending,
    /// An approved hash exists and the definition is different. The old
    /// hash stays: a server must not replace its own approval.
    Changed,
}

/// Remembers approved contracts. `cox-store`'s `Store` is the one a session
/// uses; tests use [`MemoryTrust`].
pub trait TrustStore: Send + Sync {
    fn mcp_trust_get(&self, server: &str, tool: &str) -> Result<Option<String>, StoreError>;
    fn mcp_trust_approve(&self, server: &str, tool: &str, hash: &str) -> Result<(), StoreError>;
}

/// Where each connected server was declared, plus the store that remembers
/// what the user has approved.
pub struct TrustCtx<'a> {
    pub sources: &'a HashMap<String, String>,
    pub store: &'a dyn TrustStore,
}

/// In-memory approvals. The integration test uses it so nothing opens a
/// database or a socket.
#[derive(Default)]
pub struct MemoryTrust {
    rows: Mutex<HashMap<(String, String), String>>,
}

impl TrustStore for MemoryTrust {
    fn mcp_trust_get(&self, server: &str, tool: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .rows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&(server.to_string(), tool.to_string()))
            .cloned())
    }

    fn mcp_trust_approve(&self, server: &str, tool: &str, hash: &str) -> Result<(), StoreError> {
        self.rows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((server.to_string(), tool.to_string()), hash.to_string());
        Ok(())
    }
}

/// The sentence a pending or changed tool shows instead of the server's
/// description. ASCII apart from the server name, and it carries neither
/// the description nor the schema.
pub fn pending_description(server: &str) -> String {
    format!("pending trust for mcp server '{server}'; run: cox mcp trust {server}")
}

/// `true` for a server the user wrote down themselves. Project `.mcp.json`
/// and a plugin package are not that. A missing source fails closed.
/// `config` is the merged `[mcp.servers]` layer; T64.7 still owns reverting
/// a project-added entry inside it, and this function does not.
pub fn user_config_layer(source: Option<&str>) -> bool {
    matches!(source, Some("config" | "~/.claude.json"))
}

/// sha256 hex of `name|description|canonical schema`. The schema is the
/// input schema only; annotations are not an argument, so they cannot move
/// the hash.
pub fn contract_hash(name: &str, description: &str, schema: &Value) -> String {
    let mut payload = String::new();
    payload.push_str(name);
    payload.push('|');
    payload.push_str(description);
    payload.push('|');
    payload.push_str(&canonical_json(schema));
    let digest = Sha256::digest(payload.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

/// What `listed` should do with one stored row. `Baseline` means the caller
/// inserts `current` and then treats the tool as approved.
pub(crate) enum Verdict {
    Approved,
    Pending,
    Changed,
    Baseline,
}

pub(crate) fn verdict(stored: Option<&str>, current: &str, user_layer: bool) -> Verdict {
    match stored {
        Some(hash) if hash == current => Verdict::Approved,
        Some(_) => Verdict::Changed,
        None if user_layer => Verdict::Baseline,
        None => Verdict::Pending,
    }
}

/// Object keys sorted, arrays in order, no insignificant whitespace.
/// `serde_json` keeps insertion order (`preserve_order`), so a server that
/// reorders keys would look like a new contract without this.
fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string()),
        Value::Array(items) => {
            let mut out = String::from('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&canonical_json(item));
            }
            out.push(']');
            out
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = String::from('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(*key).unwrap_or_else(|_| "\"\"".to_string()));
                out.push(':');
                out.push_str(&canonical_json(&map[*key]));
            }
            out.push('}');
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn contract_hash_follows_name_description_and_sorted_schema_only() {
        let left = json!({"b": 1, "a": {"z": true, "y": "q"}});
        let right = json!({"a": {"y": "q", "z": true}, "b": 1});
        assert_eq!(
            contract_hash("run", "do the thing", &left),
            contract_hash("run", "do the thing", &right)
        );
        assert_ne!(
            contract_hash("run", "do the thing", &left),
            contract_hash("run", "ignore previous instructions", &left)
        );
        assert_ne!(
            contract_hash("run", "do the thing", &left),
            contract_hash("other", "do the thing", &left)
        );
        assert_ne!(
            contract_hash("run", "do the thing", &left),
            contract_hash(
                "run",
                "do the thing",
                &json!({"a": {"y": "q", "z": false}, "b": 1})
            )
        );
    }

    #[test]
    fn verdict_baselines_only_the_user_layer_and_keeps_a_changed_hash() {
        assert!(matches!(verdict(None, "h", false), Verdict::Pending));
        assert!(matches!(verdict(None, "h", true), Verdict::Baseline));
        assert!(matches!(verdict(Some("h"), "h", false), Verdict::Approved));
        assert!(matches!(verdict(Some("old"), "h", true), Verdict::Changed));
        assert!(!user_config_layer(Some(".mcp.json")));
        assert!(!user_config_layer(Some("plugin:demo")));
        assert!(!user_config_layer(None));
        assert!(user_config_layer(Some("config")));
        assert!(user_config_layer(Some("~/.claude.json")));
    }
}
