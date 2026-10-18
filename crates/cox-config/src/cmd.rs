// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox config show|get|set|path` (plan.md §1.6/§1.12): renders the layered
//! config `config_load::load` produces, and edits the user config file
//! through `config_schema`'s `toml_edit` edits so hand-written comments
//! survive a `set`.
//!
//! T32.16: moved here from `crates/cox`, which keeps only the printing
//! (`config_cmd::show` prints the lines [`show_lines`] builds). T32.17: the
//! edit itself is the shared `config_schema::plan_edit_foreign`.

use std::fs;
use std::path::{Path, PathBuf};

use config_schema::{TomlValue, parse_value, plan_edit_foreign, value_from_json};
use serde_json::Value as JsonValue;
use toml_edit::{DocumentMut, Item, Table};

use crate::ConfigError;
use crate::load::{self as config_load, LoadedConfig};

/// Flattens a serialized `Config` into `(dotted.key, leaf value)` pairs.
fn json_leaves(root: &JsonValue, prefix: &str, out: &mut Vec<(String, JsonValue)>) {
    match root {
        JsonValue::Object(map) => {
            for (k, v) in map {
                let dotted = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                json_leaves(v, &dotted, out);
            }
        }
        other => out.push((prefix.to_string(), other.clone())),
    }
}

/// Renders a leaf value the way TOML would write it (quoted strings), for
/// `cox config show`.
fn fmt_toml_value(v: &JsonValue) -> String {
    match v {
        JsonValue::String(s) => format!("{s:?}"),
        JsonValue::Number(n) => n.to_string(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Null => "\"\"".to_string(),
        JsonValue::Array(items) => {
            let rendered: Vec<String> = items.iter().map(fmt_toml_value).collect();
            format!("[{}]", rendered.join(", "))
        }
        // Only reachable for a hook/mcp-server entry with nested tables
        // (e.g. `[[hooks.PreToolUse]]`), which `default.toml` never has.
        JsonValue::Object(_) => serde_json::to_string(v).unwrap_or_default(),
    }
}

/// Renders a leaf value bare (unquoted strings), for `cox config get`.
fn fmt_plain_value(v: &JsonValue) -> String {
    match v {
        JsonValue::String(s) => s.clone(),
        other => fmt_toml_value(other),
    }
}

/// Every `(dotted.key, value)` leaf of the effective config, sorted by key:
/// what `cox config show` prints and the desktop Settings screen lists.
pub fn leaves(loaded: &LoadedConfig) -> Result<Vec<(String, JsonValue)>, ConfigError> {
    let json = serde_json::to_value(&loaded.config)?;
    let mut leaves = Vec::new();
    json_leaves(&json, "", &mut leaves);
    leaves.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(leaves)
}

/// The lines `cox config show [--sources]` prints, in order.
pub fn show_lines(loaded: &LoadedConfig, with_sources: bool) -> Result<Vec<String>, ConfigError> {
    let leaves = leaves(loaded)?;
    let mut lines = Vec::with_capacity(leaves.len());
    for (key, value) in leaves {
        let rendered = fmt_toml_value(&value);
        if with_sources {
            lines.push(format!("{key} = {rendered}  # {}", loaded.source_of(&key)));
        } else {
            lines.push(format!("{key} = {rendered}"));
        }
    }
    Ok(lines)
}

/// `cox config get <key>`. `None` if the key doesn't exist in the schema.
pub fn get(loaded: &LoadedConfig, key: &str) -> Option<String> {
    let json = serde_json::to_value(&loaded.config).ok()?;
    let mut cur = &json;
    for part in key.split('.') {
        cur = cur.get(part)?;
    }
    Some(fmt_plain_value(cur))
}

/// `cox config path`.
pub fn path() -> PathBuf {
    config_load::user_config_path()
}

/// `cox config set <key> <value>`: edits the user config file in place,
/// keeping comments and formatting for everything it doesn't touch; creates
/// the file (and `~/.cox`) if absent. The value is TOML (`5`, `true`,
/// `"text"`, `[1, 2]`), or a bare string for input that isn't valid TOML on
/// its own (`cox config set tiers.code.model sonnet`). Returns the path
/// written.
pub fn set(key: &str, raw_value: &str) -> Result<PathBuf, ConfigError> {
    let path = config_load::user_config_path();
    set_value_in(&path, key, parse_value(raw_value))?;
    Ok(path)
}

/// [`set`] for the desktop app (T37.30): the value arrives as JSON, typed
/// already, so nothing falls back to a bare string, and the file is the
/// app's own `config.toml`.
pub fn set_json_in(path: &Path, key: &str, value: &JsonValue) -> Result<(), ConfigError> {
    set_value_in(path, key, value_from_json(key, value)?)
}

/// Writes `contents` to `path`, creating its directory first: the TUI theme
/// editor's save (T46.7).
pub fn write_file(path: &Path, contents: &str) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}

/// The edit is checked the way the loader reads the user file, through
/// [`config_load::check_user_toml`] rather than `config_schema::plan_edit`'s
/// own type check, so a refusal is the loader's `CoreError` (T22.12).
fn set_value_in(path: &Path, key: &str, value: TomlValue) -> Result<(), ConfigError> {
    let edit = plan_edit_foreign(path, &[(key, value)])?;
    config_load::check_user_toml(&edit.after)?;
    Ok(edit.apply()?)
}

/// Appends one rule string to `permissions.allow` in the config file at
/// `path` (T69.2): `Ok(true)` when the file changed, `Ok(false)` when the
/// rule was already there. Unlike [`set`], which a person runs once at a
/// terminal, this runs at an approval prompt while other cox sessions may be
/// doing the same, so the read-edit-write holds an advisory lock on a
/// sibling file and the new contents land through a temp file and a rename:
/// two sessions cannot interleave into a lost rule or a half-written file.
pub fn append_allow_rule_in(path: &Path, rule: &str) -> Result<bool, ConfigError> {
    const KEY: &str = "permissions.allow";
    // A dotfiles setup symlinks `config.toml`; renaming over the link would
    // replace it with a regular file, so the edit lands on the target.
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let name = target.file_name().map_or_else(
        || "config.toml".into(),
        |n| n.to_string_lossy().into_owned(),
    );
    // The lock is a separate file: the rename swaps the config's inode, and a
    // lock on the old inode would guard nothing.
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(target.with_file_name(format!(".{name}.lock")))?;
    lock.lock()?;

    let existing = match fs::read_to_string(&target) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    let mut doc: DocumentMut =
        existing
            .parse()
            .map_err(|error: toml_edit::TomlError| ConfigError::InvalidToml {
                path: target.clone(),
                message: error.to_string(),
            })?;
    let permissions = doc
        .as_table_mut()
        .entry("permissions")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| ConfigError::NotATable {
            part: "permissions".into(),
            key: KEY.into(),
        })?;
    let allow = permissions
        .entry("allow")
        .or_insert(Item::Value(TomlValue::Array(Default::default())))
        .as_array_mut()
        .ok_or_else(|| ConfigError::NotAnArray { key: KEY.into() })?;
    if allow.iter().any(|v| v.as_str() == Some(rule)) {
        return Ok(false);
    }
    // One rule per line, like the list a person would write by hand; an
    // array the user kept inline stays inline.
    let multiline = allow.is_empty()
        || allow.iter().any(|v| {
            v.decor()
                .prefix()
                .and_then(|p| p.as_str())
                .is_some_and(|p| p.contains('\n'))
        });
    let mut value = TomlValue::from(rule);
    if multiline {
        value.decor_mut().set_prefix("\n    ");
        allow.set_trailing_comma(true);
        allow.set_trailing("\n");
    }
    allow.push_formatted(value);

    let contents = doc.to_string();
    config_load::check_user_toml(&contents)?;
    let temp = target.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let written = fs::write(&temp, &contents).and_then(|()| fs::rename(&temp, &target));
    if let Err(e) = written {
        let _ = fs::remove_file(&temp);
        return Err(e.into());
    }
    Ok(true)
}

#[cfg(test)]
// why: env::set_var/remove_var are unsafe in edition 2024 (per-process tests).
#[allow(unsafe_code)]
mod tests {
    use cox_protocol::CoreError;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn config_set_preserves_comments() {
        let _guard = crate::load::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempdir().expect("tempdir");
        // Point `COX_HOME` at a scratch dir so `set` writes there, not the
        // developer's real `~/.cox`.
        unsafe { std::env::set_var("COX_HOME", home.path()) };

        let path = home.path().join("config.toml");
        fs::write(
            &path,
            "# my notes on this file\n[tiers.code]\n# pinned deliberately\nmodel = \"claude-sonnet-5\"\n",
        )
        .expect("seed user config");

        set("tiers.code.model", "claude-opus-5").expect("set succeeds");
        let after = fs::read_to_string(&path).expect("read back");

        unsafe { std::env::remove_var("COX_HOME") };

        assert!(after.contains("# my notes on this file"));
        assert!(after.contains("# pinned deliberately"));
        assert!(after.contains("model = \"claude-opus-5\""));
        assert!(!after.contains("claude-sonnet-5"));
    }

    #[test]
    fn config_set_creates_missing_file_and_parents() {
        let _guard = crate::load::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempdir().expect("tempdir");
        let nested = home.path().join("nested-home");
        unsafe { std::env::set_var("COX_HOME", &nested) };

        let path = set("budget.session_usd", "10").expect("set succeeds");

        unsafe { std::env::remove_var("COX_HOME") };

        assert_eq!(path, nested.join("config.toml"));
        let contents = fs::read_to_string(&path).expect("file created");
        assert!(contents.contains("session_usd = 10"));
    }

    /// `set` then `load` in a scratch `COX_HOME`: the value `set` wrote is
    /// what the effective config carries. `Err` is the load error.
    fn set_then_load(key: &str, value: &str) -> Result<LoadedConfig, CoreError> {
        let home = tempdir().expect("tempdir");
        let cwd = tempdir().expect("tempdir");
        let mut result = None;
        crate::load::temp_env(&[("COX_HOME", home.path().to_str())], || {
            result = Some(match set(key, value) {
                Ok(_) => {
                    config_load::load(cwd.path(), &JsonValue::Object(Default::default()), |_| None)
                }
                Err(ConfigError::Rejected(rejected)) => Err(rejected),
                Err(other) => panic!("set {key} = {value}: {other}"),
            });
        });
        result.expect("temp_env ran the closure")
    }

    #[test]
    fn set_refuses_out_of_range_value_and_leaves_file_untouched() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(&path, "# mine\n[desktop.appearance]\ndepth = 0.5\n").expect("seed");
        let before = fs::read(&path).expect("read");

        match set_value_in(&path, "desktop.appearance.depth", parse_value("1.5")) {
            Err(ConfigError::Rejected(CoreError::Config { key, message })) => {
                assert_eq!(key, "desktop.appearance.depth");
                assert!(message.contains("out of range"), "{message}");
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
        assert_eq!(fs::read(&path).expect("read"), before);
    }

    /// T32.17: the shared edit keeps a key's trailing comment, edits an
    /// inline table in place and refuses an empty key part, where the old
    /// table walk dropped the comment and refused the inline table.
    #[test]
    fn set_keeps_a_trailing_comment_and_edits_an_inline_table() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[budget]\nsession_usd = 1 # mine\n[tiers]\ncode = { model = \"a\" }\n",
        )
        .expect("seed");
        set_value_in(&path, "budget.session_usd", parse_value("2")).expect("set budget");
        set_value_in(&path, "tiers.code.model", parse_value("b")).expect("set inline");
        let written = fs::read_to_string(&path).expect("read back");
        assert!(written.contains("session_usd = 2 # mine"), "{written}");
        assert!(written.contains("code = { model = \"b\" }"), "{written}");
        let empty = set_value_in(&path, "budget..x", parse_value("1"));
        assert!(matches!(empty, Err(ConfigError::EmptyKey)), "{empty:?}");
    }

    #[test]
    fn set_json_writes_typed_values_and_rejects_objects() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        set_json_in(&path, "budget.session_usd", &serde_json::json!(7.5)).expect("number");
        set_json_in(&path, "tiers.code.model", &serde_json::json!("5")).expect("string");
        set_json_in(&path, "core.workspace_roots", &serde_json::json!(["a/b"])).expect("list");
        let written = fs::read_to_string(&path).expect("read back");
        assert!(written.contains("session_usd = 7.5"), "{written}");
        assert!(written.contains("model = \"5\""), "{written}");
        assert!(written.contains("workspace_roots = [\"a/b\"]"), "{written}");
        let object = set_json_in(&path, "mcp.servers", &serde_json::json!({ "x": 1 }));
        assert!(matches!(object, Err(ConfigError::UnsupportedValue { .. })));
    }

    #[test]
    fn config_set_desktop_material_round_trips() {
        let loaded = set_then_load("desktop.appearance.material", "glossy").expect("load succeeds");
        let appearance = &loaded.config.desktop.appearance;
        assert_eq!(appearance.material, cox_protocol::config::Material::Glossy);
        assert_eq!(loaded.source_of("desktop.appearance.material"), "user");
        assert_eq!(
            appearance.opacity,
            cox_protocol::config::DESKTOP_DEFAULT_OPACITY
        );
        assert!(loaded.config.desktop.transcript.cross_block_selection);
    }

    /// A104: per turn unless the user picks the session's.
    #[test]
    fn config_set_desktop_cache_hit_round_trips() {
        use cox_protocol::config::CacheHitScope;
        assert_eq!(
            cox_protocol::Config::default().desktop.context.cache_hit,
            CacheHitScope::Turn
        );
        let loaded = set_then_load("desktop.context.cache_hit", "session").expect("load succeeds");
        assert_eq!(
            loaded.config.desktop.context.cache_hit,
            CacheHitScope::Session
        );
        assert!(set_then_load("desktop.context.cache_hit", "call").is_err());
    }

    /// A108: review comments queue while a turn runs unless the user picks now.
    #[test]
    fn config_set_desktop_review_send_round_trips() {
        use cox_protocol::config::ReviewSend;
        assert_eq!(
            cox_protocol::Config::default().desktop.review.send,
            ReviewSend::Queue
        );
        let loaded = set_then_load("desktop.review.send", "now").expect("load succeeds");
        assert_eq!(loaded.config.desktop.review.send, ReviewSend::Now);
        assert_eq!(loaded.source_of("desktop.review.send"), "user");
        assert!(set_then_load("desktop.review.send", "later").is_err());
    }

    /// T51.14: the menu-bar extra is on unless the user turns it off.
    #[test]
    fn config_set_desktop_menu_bar_round_trips() {
        assert!(cox_protocol::Config::default().desktop.menu_bar);
        let loaded = set_then_load("desktop.menu_bar", "false").expect("load succeeds");
        assert!(!loaded.config.desktop.menu_bar);
        assert_eq!(loaded.source_of("desktop.menu_bar"), "user");
        assert!(set_then_load("desktop.menu_bar", "sometimes").is_err());
    }

    /// A113: `default.toml` names sessions; the user can turn it off.
    #[test]
    fn config_set_session_auto_title_round_trips() {
        assert!(!cox_protocol::Config::default().session.auto_title);
        let loaded = set_then_load("session.auto_title", "false").expect("load succeeds");
        assert!(!loaded.config.session.auto_title);
        assert_eq!(loaded.source_of("session.auto_title"), "user");
        let loaded = set_then_load("tui.theme", "auto").expect("load succeeds");
        assert!(loaded.config.session.auto_title);
        assert!(set_then_load("session.auto_title", "sometimes").is_err());
    }

    /// A109: no dark highlight on controls unless the user picks the subtle one or every level.
    #[test]
    fn config_set_desktop_dark_highlight_round_trips() {
        use cox_protocol::config::{DarkHighlight, DarkHighlightScope};
        let defaults = cox_protocol::Config::default().desktop.appearance;
        assert_eq!(defaults.dark_highlight, DarkHighlight::None);
        assert_eq!(defaults.dark_highlight_scope, DarkHighlightScope::Controls);
        let loaded =
            set_then_load("desktop.appearance.dark_highlight", "subtle").expect("load succeeds");
        assert_eq!(
            loaded.config.desktop.appearance.dark_highlight,
            DarkHighlight::Subtle
        );
        let loaded =
            set_then_load("desktop.appearance.dark_highlight_scope", "all").expect("load succeeds");
        assert_eq!(
            loaded.config.desktop.appearance.dark_highlight_scope,
            DarkHighlightScope::All
        );
        assert!(set_then_load("desktop.appearance.dark_highlight", "bright").is_err());
        assert!(set_then_load("desktop.appearance.dark_highlight_scope", "e2").is_err());
    }

    /// T60.8: full glare unless the user turns it down; the bounds load, beyond them is refused.
    #[test]
    fn config_set_desktop_specular_round_trips() {
        assert_eq!(
            cox_protocol::Config::default().desktop.appearance.specular,
            cox_protocol::config::DESKTOP_DEFAULT_SPECULAR
        );
        let loaded = set_then_load("desktop.appearance.specular", "0.5").expect("load succeeds");
        assert_eq!(loaded.config.desktop.appearance.specular, 0.5);
        assert_eq!(loaded.source_of("desktop.appearance.specular"), "user");
        for bound in ["0", "1"] {
            set_then_load("desktop.appearance.specular", bound).expect("the bound loads");
        }
    }

    #[test]
    fn config_rejects_out_of_range_desktop_values() {
        for (key, value) in [
            ("desktop.appearance.opacity", "1.5"),
            ("desktop.appearance.depth", "-0.1"),
            ("desktop.appearance.specular", "1.5"),
            ("desktop.appearance.blur", "61"),
            ("desktop.transcript.text_size", "9.5"),
            ("desktop.transcript.line_height", "2.6"),
        ] {
            match set_then_load(key, value) {
                Err(CoreError::Config { key: at, message }) => {
                    assert_eq!(at, key);
                    assert!(message.contains("out of range"), "{message}");
                }
                Err(other) => panic!("{key} = {value}: wrong error {other:?}"),
                Ok(_) => panic!("{key} = {value} must be rejected"),
            }
        }
        let loaded = set_then_load("desktop.appearance.blur", "60").expect("the bound loads");
        assert_eq!(loaded.config.desktop.appearance.blur, 60.0);
        let loaded = set_then_load("desktop.transcript.text_size", "24").expect("the bound loads");
        assert_eq!(loaded.config.desktop.transcript.text_size, 24.0);
    }

    #[test]
    fn allow_rule_append_creates_the_file_and_adds_one_line() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("config.toml");
        assert!(append_allow_rule_in(&path, "Bash(git status:*)").expect("append"));
        let written = fs::read_to_string(&path).expect("read back");
        assert_eq!(
            written,
            "[permissions]\nallow = [\n    \"Bash(git status:*)\",\n]\n"
        );
        config_load::check_user_toml(&written).expect("the file still loads");
    }

    #[test]
    fn allow_rule_append_skips_a_duplicate_and_keeps_comments() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "# mine\n[permissions]\n# keep\nallow = [\"Read(src/**)\"]\n",
        )
        .expect("seed");
        assert!(append_allow_rule_in(&path, "Bash(cargo test:*)").expect("first"));
        let once = fs::read_to_string(&path).expect("read");
        assert!(!append_allow_rule_in(&path, "Bash(cargo test:*)").expect("second"));
        assert_eq!(fs::read_to_string(&path).expect("read"), once);
        assert!(once.contains("# mine") && once.contains("# keep"), "{once}");
        assert_eq!(once.matches("Bash(cargo test:*)").count(), 1);
        // An inline array stays inline.
        assert!(
            once.contains("allow = [\"Read(src/**)\", \"Bash(cargo test:*)\"]"),
            "{once}"
        );
    }

    #[test]
    fn allow_rule_append_refuses_a_non_array_and_leaves_the_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(&path, "[permissions]\nallow = \"Bash\"\n").expect("seed");
        let before = fs::read(&path).expect("read");
        let err = append_allow_rule_in(&path, "Bash(ls:*)");
        assert!(
            matches!(err, Err(ConfigError::NotAnArray { .. })),
            "{err:?}"
        );
        assert_eq!(fs::read(&path).expect("read"), before);
    }

    #[test]
    fn allow_rule_append_from_many_threads_loses_nothing() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::thread::scope(|s| {
            for n in 0..8 {
                let path = &path;
                s.spawn(move || {
                    append_allow_rule_in(path, &format!("Bash(tool{n}:*)")).expect("append");
                });
            }
        });
        let written = fs::read_to_string(&path).expect("read back");
        for n in 0..8 {
            assert_eq!(
                written.matches(&format!("Bash(tool{n}:*)")).count(),
                1,
                "{written}"
            );
        }
    }
}
