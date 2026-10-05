//! The one config owner (T32.16; `docs/design/crates.md` C16): `load`
//! layers `config/default.toml`, the user and project `config.toml`, the
//! `.claude/settings.json` import, `COX_*` env vars and CLI flags, applies
//! the project-config guard list and answers provenance; `cmd` renders and
//! edits the user config file for `cox config show|get|set|path`; [`schema`]
//! is `Config`'s JSON Schema, which the drift test pins to
//! `docs/config.jsonschema` and the desktop Settings screen reads.
//!
//! Separate from `crates/cox` (size (c): a ~1k-line leaf, and reuse (d):
//! any surface can load config without the clap binary). `figment` and
//! `toml_edit` live here only. Errors are [`ConfigError`], because AGENTS.md
//! keeps `anyhow` in `crates/cox`; the binary maps them at its call sites.
//! `crates/cox` keeps the clap-facing glue (the flag layer built from `Cli`,
//! the Claude-settings reader from `cox-ext`, the keymap for `cox-tui`, and
//! all printing) and re-exports the rest at its old `config_load` and
//! `config_cmd` paths.

// why: this crate was not under `missing_docs` before the workspace lints;
// drop this allow once its public items are documented.
#![allow(missing_docs)]

use std::path::PathBuf;

pub mod cmd;
pub mod load;

/// Why a `cox config` edit or render failed. The messages match what the
/// binary printed before T32.16 (it used `anyhow!` with the same text), so
/// `cox config set` errors read the same.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Creating `~/.cox` or writing `config.toml` failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Serializing the effective `Config` for `cox config show` failed.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// The user config file on disk is not valid TOML. `error` is not a
    /// `#[source]`: the message already embeds it, as before.
    #[error("{} is not valid TOML: {error}", path.display())]
    InvalidToml {
        path: PathBuf,
        error: toml_edit::TomlError,
    },
    /// `cox config set` got an empty key.
    #[error("empty key")]
    EmptyKey,
    /// A dotted key walks through a value that is not a table.
    #[error("`{part}` in `{key}` is not a table")]
    NotATable { part: String, key: String },
    /// A value that has no TOML form a config key takes (`null`, an object).
    #[error("`{key}` cannot be set to {value}")]
    UnsupportedValue { key: String, value: String },
}

/// `Config`'s JSON Schema, the one `docs/config.jsonschema` commits.
/// Through `Serialize`, not `Schema::to_value`: schemars orders the
/// keywords (`$schema`, `title`, …) only when it serializes.
pub fn schema() -> Result<serde_json::Value, ConfigError> {
    Ok(serde_json::to_value(schemars::schema_for!(
        cox_protocol::Config
    ))?)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// The config-schema drift test (AGENTS.md "Config files"): generates
    /// `Config`'s JSON Schema and checks it against the committed
    /// `docs/config.jsonschema`, so a config shape change is a reviewable
    /// diff. Same shape as `cox-protocol`'s `protocol_jsonschema` test.
    #[test]
    fn config_jsonschema_matches_committed_file() {
        let generated = serde_json::to_string_pretty(&super::schema().expect("schema"))
            .expect("schema serializes")
            + "\n";

        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/config.jsonschema");
        match std::fs::read_to_string(&path) {
            Ok(committed) => assert_eq!(
                committed, generated,
                "docs/config.jsonschema is stale; regenerate it (see this test) and commit it"
            ),
            Err(_) => {
                // First run: create it. `git status` will show it as new/changed for review.
                std::fs::write(&path, &generated).expect("write docs/config.jsonschema");
            }
        }
    }

    /// T52.2: the `[external_agents.<name>]` part of the committed schema
    /// matches the types, and an entry has exactly the four keys the docs
    /// name. Narrower than the whole-file test above, so a drift here names
    /// the table that moved.
    #[test]
    fn external_agents_schema_drift() {
        let generated = super::schema().expect("schema");
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/config.jsonschema");
        let committed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read schema"))
                .expect("committed schema is JSON");
        for pointer in ["/properties/external_agents", "/$defs/ExternalAgentConfig"] {
            assert_eq!(
                committed.pointer(pointer),
                generated.pointer(pointer),
                "{pointer} is stale in docs/config.jsonschema"
            );
        }
        let keys: Vec<&str> = generated
            .pointer("/$defs/ExternalAgentConfig/properties")
            .and_then(serde_json::Value::as_object)
            .expect("entry properties")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["command", "args", "key_env", "writable"]);
    }
}
