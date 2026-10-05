// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The Settings screen's model (DT§5.7, T37.30): every leaf of the
//! effective config with the layer it came from, what `Config`'s JSON Schema
//! says about it, and whether an edit to the user file would take effect.
//! Edits go through `cox-config`'s comment-preserving `set`. Here rather
//! than in Swift so the layering, the schema walk and the read-only rule are
//! tested once, in Rust; the loading itself stays `cox-config`'s.

use std::fs;
use std::path::{Path, PathBuf};

use cox_config::ConfigError;
use cox_config::load::LoadedConfig;
use cox_protocol::errors::CoreError;
use serde::Serialize;
use serde_json::Value;

use crate::mcp_login::McpServer;
use crate::permissions::{PermissionRule, SessionGrant};
use crate::settings_fields::{self as fields, SettingControl, SettingInput, SettingsGroup};

/// The layer a value came from, the badge beside each field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Layer {
    Default,
    User,
    Project,
    ClaudeSettings,
    Env,
    Flag,
}

impl Layer {
    fn from_source(source: &str) -> Self {
        match source {
            "user" => Self::User,
            "project" => Self::Project,
            "claude-settings" => Self::ClaudeSettings,
            "env" => Self::Env,
            "flag" => Self::Flag,
            _ => Self::Default,
        }
    }
}

impl std::fmt::Display for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Default => "default",
            Self::User => "user",
            Self::Project => "project",
            Self::ClaudeSettings => "claude-settings",
            Self::Env => "env",
            Self::Flag => "flag",
        })
    }
}

/// The control a field takes, from its schema.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SettingKind {
    Toggle,
    Integer {
        min: Option<f64>,
        max: Option<f64>,
    },
    Number {
        min: Option<f64>,
        max: Option<f64>,
    },
    Text,
    Choice {
        options: Vec<String>,
    },
    List,
    /// A shape the schema leaves open (plugin tables, hook entries).
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Setting {
    /// Dotted, as `cox config set` takes it.
    pub key: String,
    pub value: Value,
    pub layer: Layer,
    /// Whether an edit to the user file takes effect: `false` once a layer
    /// above it (project, Claude settings, env, flag) sets the key.
    pub editable: bool,
    pub kind: SettingKind,
    pub description: String,
    /// The page it is on (DT§5.7).
    pub group: SettingsGroup,
    /// Its label, `base_url` → `Base url`.
    pub title: String,
    /// The box it sits in, its table; `None` for a rule list.
    pub table: Option<String>,
    /// For a `providers.<name>` table's key, the section whose key the box takes.
    pub provider: Option<String>,
    /// `Set in <project file>` for a project value, else the schema's help.
    pub detail: Option<String>,
    /// What the field shows, from its kind and value.
    pub control: SettingControl,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SettingsView {
    /// Sorted by key.
    pub settings: Vec<Setting>,
    /// Where edits go.
    pub user_file: PathBuf,
    /// The project's `.cox/config.toml`, when there is one.
    pub project_file: Option<PathBuf>,
    /// The MCP servers in effect for `cwd` and their logins (T37.30.3).
    pub mcp: Vec<McpServer>,
    /// Project values the guard list threw out (T37.30.4).
    pub dropped: Vec<Dropped>,
    /// The allow/ask/deny rules in effect, deny first (T37.45.3).
    pub rules: Vec<PermissionRule>,
    /// The grants of the sessions open here; `App::settings` adds them.
    pub grants: Vec<SessionGrant>,
    /// The provider sections a key can be stored for, sorted.
    pub providers: Vec<String>,
}

/// A value the project's `.cox/config.toml` set and the guard list threw
/// out (`plan.md` §1.6), so the person sees why their setting still holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Dropped {
    /// Dotted; `mcp.servers.*.sandbox` names the servers in `value`.
    pub key: String,
    /// What the project set.
    pub value: String,
    /// What is in effect instead.
    pub kept: String,
    pub reason: String,
    /// The page it is listed on.
    pub group: SettingsGroup,
    /// `999 → 5`.
    pub change: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error(transparent)]
    Load(#[from] CoreError),
    #[error(transparent)]
    Edit(#[from] ConfigError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("not JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("no setting `{0}`")]
    Unknown(String),
    #[error("`{key}` is set by the {layer} layer; change it there")]
    ReadOnly { key: String, layer: Layer },
    /// `cox_permission`'s grammar refused it (T37.45.3).
    #[error("`{rule}` is not a rule: {message}")]
    Rule { rule: String, message: String },
    #[error("no rule `{0}`")]
    NoRule(String),
    #[error("`{0}` takes no number JSON cannot carry")]
    NotFinite(String),
}

/// The effective config for a session in `cwd`, loaded as `live.rs` loads
/// it: no flags, no Claude-settings layer (only `crates/cox` reads that).
pub fn load(user_file: &Path, cwd: &Path) -> Result<LoadedConfig, SettingsError> {
    let flags = Value::Object(serde_json::Map::new());
    Ok(cox_config::load::load_in(user_file, cwd, &flags, |_| None)?)
}

pub fn view(user_file: &Path, cwd: &Path) -> Result<SettingsView, SettingsError> {
    view_of(&load(user_file, cwd)?, user_file, cwd)
}

/// The view of an already loaded config. `mcp` is left empty: reading a
/// server's token may wait on the keychain, so `App::settings` adds them.
pub fn view_of(
    loaded: &LoadedConfig,
    user_file: &Path,
    cwd: &Path,
) -> Result<SettingsView, SettingsError> {
    let schema = cox_config::schema()?;
    let project_file = cox_config::load::project_config_path(cwd).filter(|p| p.exists());
    let models = crate::models::choices(&loaded.config);
    let settings: Vec<Setting> = cox_config::cmd::leaves(loaded)?
        .into_iter()
        .map(|(key, value)| {
            let layer = Layer::from_source(loaded.source_of(&key));
            let (kind, description) = describe(&schema, &key);
            Setting {
                editable: matches!(layer, Layer::Default | Layer::User),
                group: SettingsGroup::of(&key),
                title: fields::title(&key),
                table: fields::table(&key),
                provider: fields::provider(&key),
                detail: fields::detail(layer, &description, project_file.as_deref()),
                control: fields::control(&key, &kind, &value, &models),
                key,
                value,
                layer,
                kind,
                description,
            }
        })
        .collect();
    Ok(SettingsView {
        rules: crate::permissions::rules(&settings),
        grants: Vec::new(),
        providers: fields::providers(settings.iter().map(|s| s.key.as_str())),
        settings,
        user_file: user_file.to_path_buf(),
        project_file,
        mcp: Vec::new(),
        dropped: loaded
            .violations
            .iter()
            .map(|v| Dropped {
                key: v.key.to_string(),
                group: SettingsGroup::of(v.key),
                change: fields::change(&v.project_value, &v.reverted_to),
                value: v.project_value.clone(),
                kept: v.reverted_to.clone(),
                reason: v.reason().to_string(),
            })
            .collect(),
    })
}

/// Writes `json` for `key` to the user file and returns the new view. An
/// edit the loader then rejects (a wrong type, out of range) puts the file
/// back as it was, so the app never leaves a config the CLI cannot load.
pub fn set(
    user_file: &Path,
    cwd: &Path,
    key: &str,
    json: &str,
) -> Result<SettingsView, SettingsError> {
    let value: Value = serde_json::from_str(json)?;
    let before = view(user_file, cwd)?;
    let row = before
        .settings
        .iter()
        .find(|s| s.key == key)
        .ok_or_else(|| SettingsError::Unknown(key.to_string()))?;
    if !row.editable {
        return Err(SettingsError::ReadOnly {
            key: key.to_string(),
            layer: row.layer,
        });
    }
    let previous = fs::read(user_file).ok();
    cox_config::cmd::set_json_in(user_file, key, &value)?;
    match view(user_file, cwd) {
        Ok(after) => Ok(after),
        Err(rejected) => {
            match previous {
                Some(bytes) => fs::write(user_file, bytes)?,
                None => fs::remove_file(user_file)?,
            }
            Err(rejected)
        }
    }
}

/// Sets `key` from what its control sent, typed by the key's kind first
/// (`settings_fields::typed`); the new view.
pub fn set_input(
    user_file: &Path,
    cwd: &Path,
    key: &str,
    input: SettingInput,
) -> Result<SettingsView, SettingsError> {
    let (kind, _) = describe(&cox_config::schema()?, key);
    let value =
        fields::typed(&kind, input).ok_or_else(|| SettingsError::NotFinite(key.to_owned()))?;
    set(user_file, cwd, key, &value.to_string())
}

/// The control and help text the schema gives `key`; a key it does not
/// reach is `Other` with no text.
fn describe(schema: &Value, key: &str) -> (SettingKind, String) {
    let Some(node) = property(schema, key) else {
        return (SettingKind::Other, String::new());
    };
    let target = resolve(schema, node);
    let description = node
        .get("description")
        .or_else(|| target.get("description"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (kind(target), description)
}

/// Walks `properties` (and a map's `additionalProperties`) down `key`.
fn property<'a>(root: &'a Value, key: &str) -> Option<&'a Value> {
    key.split('.').try_fold(root, |node, part| {
        let node = resolve(root, node);
        node.get("properties")
            .and_then(|p| p.get(part))
            .or_else(|| node.get("additionalProperties").filter(|v| v.is_object()))
    })
}

/// Follows `#/$defs/<name>` references, a bounded number of hops.
fn resolve<'a>(root: &'a Value, mut node: &'a Value) -> &'a Value {
    for _ in 0..8 {
        let next = node
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|r| r.strip_prefix("#/$defs/"))
            .and_then(|name| root.get("$defs")?.get(name));
        match next {
            Some(next) => node = next,
            None => break,
        }
    }
    node
}

fn kind(node: &Value) -> SettingKind {
    let consts: Option<Vec<String>> = node.get("oneOf").and_then(Value::as_array).map(|all| {
        all.iter()
            .filter_map(|v| v.get("const")?.as_str().map(String::from))
            .collect()
    });
    if let Some(options) = consts.filter(|o| !o.is_empty()) {
        return SettingKind::Choice { options };
    }
    let bound = |name| node.get(name).and_then(Value::as_f64);
    let (min, max) = (bound("minimum"), bound("maximum"));
    let ty = match node.get("type") {
        Some(Value::Array(types)) => types
            .iter()
            .filter_map(Value::as_str)
            .find(|t| *t != "null"),
        Some(other) => other.as_str(),
        None => None,
    };
    match ty {
        Some("boolean") => SettingKind::Toggle,
        Some("integer") => SettingKind::Integer { min, max },
        Some("number") => SettingKind::Number { min, max },
        Some("string") => SettingKind::Text,
        Some("array") => SettingKind::List,
        _ => SettingKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A user file and a git checkout whose `.cox/config.toml` overrides
    /// the model and tries to raise the budget (a guarded key).
    fn scratch() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let user = dir.path().join("home/config.toml");
        let project = dir.path().join("project");
        fs::create_dir_all(user.parent().expect("parent")).expect("home");
        fs::create_dir_all(project.join(".git")).expect(".git");
        fs::create_dir_all(project.join(".cox")).expect(".cox");
        fs::write(
            &user,
            "# mine\n[tiers.code]\nmodel = \"user-model\"\n\n[desktop.appearance]\nmaterial = \"glossy\"\n",
        )
        .expect("user file");
        fs::write(
            project.join(".cox/config.toml"),
            "[tiers.code]\nmodel = \"project-model\"\n\n[budget]\nsession_usd = 999.0\n",
        )
        .expect("project file");
        (dir, user, project)
    }

    fn rows<'a>(view: &'a SettingsView, keys: &[&str]) -> Vec<&'a Setting> {
        keys.iter()
            .map(|k| {
                view.settings
                    .iter()
                    .find(|s| s.key == *k)
                    .expect("key listed")
            })
            .collect()
    }

    #[test]
    fn a_setting_the_project_overrides_is_read_only_with_its_layer() {
        let (dir, user, project) = scratch();
        let view = view(&user, &project).expect("view");
        assert!(view.project_file.is_some());
        // The scratch directory differs per run; the snapshot names it `<tmp>`.
        let tmp = fs::canonicalize(dir.path())
            .expect("canonical")
            .display()
            .to_string();
        let shown: Vec<Setting> = rows(
            &view,
            &[
                "tiers.code.model",
                "budget.session_usd",
                "desktop.appearance.material",
                "desktop.appearance.opacity",
                "desktop.appearance.tint",
                "core.max_turns",
                "core.workspace_roots",
            ],
        )
        .into_iter()
        .map(|row| Setting {
            detail: row.detail.as_ref().map(|d| d.replace(&tmp, "<tmp>")),
            // The catalog's rows change with each vendored update; the
            // project's unlisted model, kept first, is what this pins.
            control: match &row.control {
                SettingControl::Menu { value, options } => SettingControl::Menu {
                    value: value.clone(),
                    options: options.iter().take(1).cloned().collect(),
                },
                other => other.clone(),
            },
            ..row.clone()
        })
        .collect();
        insta::assert_json_snapshot!(shown);
    }

    #[test]
    fn a_project_value_the_guard_drops_is_listed_with_its_reason() {
        let (_dir, user, project) = scratch();
        let view = view(&user, &project).expect("view");
        insta::assert_json_snapshot!(view.dropped);
    }

    #[test]
    fn a_dropped_value_says_what_replaced_it() {
        let (_dir, user, project) = scratch();
        let view = view(&user, &project).expect("view");
        let budget = view
            .dropped
            .iter()
            .find(|d| d.key == "budget.session_usd")
            .expect("dropped");
        assert_eq!(budget.group, SettingsGroup::Budget);
        assert_eq!(budget.change, format!("{} → {}", budget.value, budget.kept));
        assert!(budget.change.starts_with("999"), "{}", budget.change);
    }

    #[test]
    fn a_typed_input_is_set_as_its_kinds_json() {
        let (_dir, user, project) = scratch();
        let text = |value: &str| SettingInput::Text {
            value: value.into(),
        };
        let after = set_input(&user, &project, "core.max_turns", text(" 12 ")).expect("set");
        let row = rows(&after, &["core.max_turns"])[0];
        assert_eq!(row.value, serde_json::json!(12));
        let nan = SettingInput::Number { value: f64::NAN };
        let err = set_input(&user, &project, "desktop.appearance.opacity", nan).expect_err("nan");
        assert!(matches!(err, SettingsError::NotFinite(_)), "{err}");
    }

    #[test]
    fn set_writes_the_user_file_and_keeps_its_comments() {
        let (_dir, user, project) = scratch();
        let after = set(&user, &project, "desktop.appearance.opacity", "0.5").expect("set");
        let row = rows(&after, &["desktop.appearance.opacity"])[0];
        assert_eq!(
            (row.layer, &row.value),
            (Layer::User, &serde_json::json!(0.5))
        );
        assert!(
            fs::read_to_string(&user)
                .expect("read")
                .starts_with("# mine\n")
        );
    }

    #[test]
    fn set_refuses_a_key_a_higher_layer_sets() {
        let (_dir, user, project) = scratch();
        let err = set(&user, &project, "tiers.code.model", "\"x\"").expect_err("read-only");
        assert!(matches!(
            err,
            SettingsError::ReadOnly {
                layer: Layer::Project,
                ..
            }
        ));
        let unknown = set(&user, &project, "no.such.key", "1").expect_err("unknown");
        assert!(matches!(unknown, SettingsError::Unknown(_)));
    }

    #[test]
    fn a_value_the_loader_rejects_leaves_the_user_file_as_it_was() {
        let (_dir, user, project) = scratch();
        let before = fs::read(&user).expect("read");
        let err = set(&user, &project, "desktop.appearance.opacity", "1.5").expect_err("range");
        assert!(matches!(err, SettingsError::Load(_)), "{err}");
        assert_eq!(fs::read(&user).expect("read"), before);
    }
}
