// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The Settings screen's layout rules (DT§5.7, T58.4.8–T58.4.9): the page
//! a key falls on, the box it sits in, its label, its detail line, the
//! control its kind and value give it, how a typed input becomes the key's
//! JSON, the provider sections a key can be stored for, and whether a typed
//! key may be stored.
//! Separate from `settings`, which loads and edits the config, because these
//! only decide what a client shows; every client reads them from here
//! instead of writing them again (T58.4).

use std::collections::BTreeSet;
use std::path::Path;

use cox_protocol::types::Tier;
use serde::Serialize;
use serde_json::Value;

use crate::models::ModelChoice;
use crate::permissions::RuleKind;
use crate::settings::{Layer, SettingKind};

/// The pages DT§5.7 names, in its order, by a key's top-level table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsGroup {
    General,
    Models,
    Permissions,
    Sandbox,
    Budget,
    Mcp,
    Plugins,
    Appearance,
    Advanced,
}

impl SettingsGroup {
    /// DT§5.7's order, the sidebar's.
    pub const ALL: [Self; 9] = [
        Self::General,
        Self::Models,
        Self::Permissions,
        Self::Sandbox,
        Self::Budget,
        Self::Mcp,
        Self::Plugins,
        Self::Appearance,
        Self::Advanced,
    ];

    /// The page `key` falls on; a table DT§5.7 does not name is Advanced.
    pub fn of(key: &str) -> Self {
        match key.split_once('.').map_or(key, |(table, _)| table) {
            "core" => Self::General,
            "tiers" | "jobs" | "providers" => Self::Models,
            "permissions" => Self::Permissions,
            "sandbox" => Self::Sandbox,
            "budget" => Self::Budget,
            "mcp" => Self::Mcp,
            "plugins" => Self::Plugins,
            "desktop" => Self::Appearance,
            _ => Self::Advanced,
        }
    }
}

/// The key's last segment in words, `base_url` → `Base url`: the label a
/// field shows and the search also matches.
pub fn title(key: &str) -> String {
    let words = key.rsplit('.').next().unwrap_or(key).replace('_', " ");
    let mut chars = words.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// The box `key` sits in, the table that holds it (`tiers.code`). A rule
/// list has none: the Permissions page shows those as its own rule box.
pub fn table(key: &str) -> Option<String> {
    if RuleKind::ALL.iter().any(|kind| kind.key() == key) {
        return None;
    }
    Some(
        key.rsplit_once('.')
            .map_or("", |(table, _)| table)
            .to_owned(),
    )
}

/// The provider section a `providers.<name>` table's own key belongs to,
/// so that box takes the provider's key; a deeper table has none.
pub fn provider(key: &str) -> Option<String> {
    match key.split('.').collect::<Vec<_>>()[..] {
        ["providers", name, _] => Some(name.to_owned()),
        _ => None,
    }
}

/// The line under a field: the project file for a value the project sets,
/// else the schema's help, else nothing.
pub fn detail(layer: Layer, description: &str, project_file: Option<&Path>) -> Option<String> {
    match project_file {
        Some(file) if layer == Layer::Project => Some(format!("Set in {}", file.display())),
        _ => (!description.is_empty()).then(|| description.to_owned()),
    }
}

/// The provider sections any key names, sorted and each once.
pub fn providers<'a>(keys: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    keys.into_iter()
        .filter_map(|key| match key.split('.').collect::<Vec<_>>()[..] {
            ["providers", name, _, ..] => Some(name.to_owned()),
            _ => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// A dropped value's change, `999 → 5`: what the project set, then what holds.
pub fn change(value: &str, kept: &str) -> String {
    format!("{value} → {kept}")
}

/// More options than this take a pop-up rather than a segmented control,
/// as mockup 18's pop-ups and segments show.
const SEGMENT_LIMIT: usize = 3;

/// Mockup 19 draws the permission mode segmented, whatever its count.
const MODE_KEY: &str = "permissions.mode";

/// A pop-up's option: the value sent, and what the menu calls it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingOption {
    pub value: String,
    pub title: String,
}

/// What a field shows. A value of another type than its kind falls back to
/// its JSON (or an empty field), so the person still sees what is set.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SettingControl {
    Toggle {
        on: bool,
    },
    /// A number the schema bounds on both ends, `min` below `max`; `text`
    /// is the value as its JSON reads.
    Slider {
        value: f64,
        min: f64,
        max: f64,
        text: String,
    },
    /// A few options side by side.
    Choice {
        value: String,
        options: Vec<String>,
    },
    /// A pop-up: more options than fit side by side, or a tier's model.
    Menu {
        value: String,
        options: Vec<SettingOption>,
    },
    /// Text, or a number typed as text; [`typed`] types it back.
    Field {
        text: String,
    },
    /// A list or an open shape, as its JSON: Settings does not edit it.
    Json {
        text: String,
    },
}

/// The control `key`'s kind and value give it; `models` is the catalog a
/// tier's model pop-up offers (`models::choices`).
pub fn control(
    key: &str,
    kind: &SettingKind,
    value: &Value,
    models: &[ModelChoice],
) -> SettingControl {
    let json = || SettingControl::Json {
        text: value.to_string(),
    };
    let text = || value.as_str().unwrap_or_default().to_owned();
    match kind {
        SettingKind::Toggle => value
            .as_bool()
            .map_or_else(json, |on| SettingControl::Toggle { on }),
        SettingKind::Number {
            min: Some(min),
            max: Some(max),
        } if min < max => value
            .as_f64()
            .map_or_else(json, |number| SettingControl::Slider {
                value: number,
                min: *min,
                max: *max,
                text: value.to_string(),
            }),
        SettingKind::Integer { .. } | SettingKind::Number { .. } => SettingControl::Field {
            text: if value.is_number() {
                value.to_string()
            } else {
                String::new()
            },
        },
        SettingKind::Text => {
            let text = text();
            model_menu(key, &text, models).unwrap_or(SettingControl::Field { text })
        }
        SettingKind::Choice { options } if options.len() > SEGMENT_LIMIT && key != MODE_KEY => {
            SettingControl::Menu {
                value: text(),
                options: options
                    .iter()
                    .map(|o| SettingOption {
                        value: o.clone(),
                        title: o.clone(),
                    })
                    .collect(),
            }
        }
        SettingKind::Choice { options } => SettingControl::Choice {
            value: text(),
            options: options.clone(),
        },
        SettingKind::List | SettingKind::Other => json(),
    }
}

/// `tiers.<tier>.model` as a pop-up of the catalog's models for that tier,
/// titled by the catalog's short name (A129), else the id; a value the
/// catalog does not list stays first, so the pop-up shows it. `None` when the catalog lists none.
fn model_menu(key: &str, value: &str, models: &[ModelChoice]) -> Option<SettingControl> {
    let ["tiers", tier, "model"] = key.split('.').collect::<Vec<_>>()[..] else {
        return None;
    };
    let tier = match tier {
        "cheap" => Tier::Cheap,
        "code" => Tier::Code,
        "think" => Tier::Think,
        _ => return None,
    };
    let mut options: Vec<SettingOption> = models
        .iter()
        .filter(|m| m.tier == tier)
        .map(|m| SettingOption {
            value: m.id.clone(),
            title: m
                .short_name
                .clone()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| m.id.clone()),
        })
        .collect();
    if options.is_empty() {
        return None;
    }
    if !value.is_empty() && !options.iter().any(|o| o.value == value) {
        let kept = SettingOption {
            value: value.to_owned(),
            title: value.to_owned(),
        };
        options.insert(0, kept);
    }
    Some(SettingControl::Menu {
        value: value.to_owned(),
        options,
    })
}

/// What a control sends back before it is typed by the key's kind.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingInput {
    Bool { value: bool },
    Integer { value: i64 },
    Number { value: f64 },
    Text { value: String },
    List { values: Vec<String> },
}

/// `input` as `kind` wants it: a whole number for an integer (a slider's
/// number rounded, text parsed), a number for a number. Text that is no
/// number goes as text, so the loader says why it is refused. `None` for a
/// number JSON cannot carry (NaN, infinity).
pub fn typed(kind: &SettingKind, input: SettingInput) -> Option<Value> {
    let input = match (kind, input) {
        (SettingKind::Integer { .. }, SettingInput::Number { value }) => SettingInput::Integer {
            // Saturates past i64's range; the loader then refuses it.
            value: value.round() as i64,
        },
        (SettingKind::Integer { .. }, SettingInput::Text { value }) => {
            match value.trim().parse::<i64>() {
                Ok(number) => SettingInput::Integer { value: number },
                Err(_) => SettingInput::Text { value },
            }
        }
        (SettingKind::Number { .. }, SettingInput::Text { value }) => {
            match value.trim().parse::<f64>() {
                Ok(number) if number.is_finite() => SettingInput::Number { value: number },
                _ => SettingInput::Text { value },
            }
        }
        (_, input) => input,
    };
    match input {
        SettingInput::Bool { value } => Some(Value::Bool(value)),
        SettingInput::Integer { value } => Some(Value::from(value)),
        SettingInput::Number { value } => serde_json::Number::from_f64(value).map(Value::Number),
        SettingInput::Text { value } => Some(Value::String(value)),
        SettingInput::List { values } => Some(Value::from(values)),
    }
}

/// Why a provider key was not stored.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("the key is empty")]
    Empty,
    /// Not a `[providers.<name>]` section of the current view.
    #[error("no provider section `{provider}`")]
    UnknownProvider { provider: String },
}

/// The key to store for `provider`, trimmed; refused when nothing is left
/// or `providers` (a view's `providers`) does not list the section.
pub fn check_key(providers: &[String], provider: &str, secret: &str) -> Result<String, KeyError> {
    let secret = secret.trim();
    if secret.is_empty() {
        return Err(KeyError::Empty);
    }
    if !providers.iter().any(|p| p == provider) {
        return Err(KeyError::UnknownProvider {
            provider: provider.to_owned(),
        });
    }
    Ok(secret.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_fall_into_dt_5_7_groups() {
        let keys = [
            "core.max_turns",
            "tiers.code.model",
            "jobs.title.model",
            "providers.anthropic.base_url",
            "permissions.mode",
            "sandbox.network",
            "budget.session_usd",
            "mcp.servers.x.url",
            "plugins.dir",
            "desktop.appearance.tint",
            "telemetry.otlp",
            "lonely",
        ];
        let groups: Vec<SettingsGroup> = keys.iter().map(|k| SettingsGroup::of(k)).collect();
        use SettingsGroup::*;
        assert_eq!(
            groups,
            [
                General,
                Models,
                Models,
                Models,
                Permissions,
                Sandbox,
                Budget,
                Mcp,
                Plugins,
                Appearance,
                Advanced,
                Advanced
            ]
        );
        assert_eq!(SettingsGroup::ALL.first(), Some(&General));
        assert_eq!(SettingsGroup::ALL.last(), Some(&Advanced));
    }

    #[test]
    fn a_title_is_the_last_segment_in_words() {
        assert_eq!(title("providers.anthropic.base_url"), "Base url");
        assert_eq!(title("tiers.code.model"), "Model");
        assert_eq!(title("lonely"), "Lonely");
    }

    #[test]
    fn rule_lists_have_no_box() {
        assert_eq!(table("permissions.allow"), None);
        assert_eq!(table("permissions.deny"), None);
        assert_eq!(table("permissions.mode").as_deref(), Some("permissions"));
        assert_eq!(table("tiers.code.model").as_deref(), Some("tiers.code"));
    }

    #[test]
    fn only_a_provider_tables_own_key_names_its_provider() {
        assert_eq!(
            provider("providers.anthropic.base_url").as_deref(),
            Some("anthropic")
        );
        assert_eq!(provider("providers.anthropic.headers.x"), None);
        assert_eq!(provider("tiers.code.model"), None);
        assert_eq!(
            providers([
                "providers.openai.base_url",
                "providers.anthropic.models",
                "providers.anthropic.headers.x",
                "tiers.code.model",
            ]),
            ["anthropic", "openai"]
        );
    }

    #[test]
    fn a_project_value_names_its_file_else_the_help() {
        let file = Path::new("/p/.cox/config.toml");
        assert_eq!(
            detail(Layer::Project, "help", Some(file)).as_deref(),
            Some("Set in /p/.cox/config.toml")
        );
        assert_eq!(
            detail(Layer::User, "help", Some(file)).as_deref(),
            Some("help")
        );
        assert_eq!(detail(Layer::Default, "", None), None);
    }

    fn choice(tier: Tier, id: &str, name: Option<&str>) -> ModelChoice {
        ModelChoice {
            tier,
            provider: "anthropic".into(),
            id: id.into(),
            display_name: name.map(String::from),
            short_name: name.map(crate::status::shorten),
            efforts: Vec::new(),
            context_window: None,
        }
    }

    #[test]
    fn a_slider_needs_both_bounds() {
        let bounded = SettingKind::Number {
            min: Some(0.0),
            max: Some(1.0),
        };
        let v = serde_json::json!(0.42);
        assert_eq!(
            control("desktop.appearance.opacity", &bounded, &v, &[]),
            SettingControl::Slider {
                value: 0.42,
                min: 0.0,
                max: 1.0,
                text: "0.42".into()
            }
        );
        let open = SettingKind::Number {
            min: Some(0.0),
            max: None,
        };
        let text = SettingControl::Field {
            text: "0.42".into(),
        };
        assert_eq!(control("k", &open, &v, &[]), text);
        let flat = SettingKind::Number {
            min: Some(1.0),
            max: Some(1.0),
        };
        assert_eq!(control("k", &flat, &v, &[]), text);
        let integer = SettingKind::Integer {
            min: Some(0.0),
            max: Some(9.0),
        };
        let five = serde_json::json!(5);
        assert_eq!(
            control("k", &integer, &five, &[]),
            SettingControl::Field { text: "5".into() }
        );
        // A value of another type: the slider's JSON, a number's empty field.
        let wrong = serde_json::json!("x");
        assert_eq!(
            control("k", &bounded, &wrong, &[]),
            SettingControl::Json {
                text: "\"x\"".into()
            }
        );
        assert_eq!(
            control("k", &open, &wrong, &[]),
            SettingControl::Field {
                text: String::new()
            }
        );
    }

    #[test]
    fn permissions_mode_stays_segmented() {
        let options: Vec<String> = ["a", "b", "c", "d"].map(String::from).to_vec();
        let kind = SettingKind::Choice {
            options: options.clone(),
        };
        let v = serde_json::json!("b");
        assert_eq!(
            control(MODE_KEY, &kind, &v, &[]),
            SettingControl::Choice {
                value: "b".into(),
                options: options.clone()
            }
        );
        let SettingControl::Menu {
            value,
            options: menu,
        } = control("tiers.code.effort", &kind, &v, &[])
        else {
            panic!("a menu past the segment limit");
        };
        assert_eq!(value, "b");
        assert_eq!(menu.len(), 4);
        let three = SettingKind::Choice {
            options: options[..3].to_vec(),
        };
        assert!(matches!(
            control("tiers.code.thinking", &three, &v, &[]),
            SettingControl::Choice { .. }
        ));
    }

    #[test]
    fn an_unlisted_model_is_kept_first() {
        let models = [
            choice(Tier::Cheap, "claude-haiku-4-5", None),
            choice(Tier::Code, "claude-sonnet-5", Some("Claude Sonnet 5")),
            choice(Tier::Code, "claude-opus-5-5", Some("Claude Opus 5.5")),
        ];
        let option = |value: &str, title: &str| SettingOption {
            value: value.into(),
            title: title.into(),
        };
        assert_eq!(
            control(
                "tiers.cheap.model",
                &SettingKind::Text,
                &serde_json::json!("my-finetune"),
                &models
            ),
            SettingControl::Menu {
                value: "my-finetune".into(),
                options: vec![
                    option("my-finetune", "my-finetune"),
                    option("claude-haiku-4-5", "claude-haiku-4-5"),
                ]
            }
        );
        assert_eq!(
            control(
                "tiers.code.model",
                &SettingKind::Text,
                &serde_json::json!("claude-sonnet-5"),
                &models
            ),
            SettingControl::Menu {
                value: "claude-sonnet-5".into(),
                options: vec![
                    option("claude-sonnet-5", "Sonnet 5"),
                    option("claude-opus-5-5", "Opus 5.5"),
                ]
            }
        );
        // No catalog models for the tier: a text field.
        assert_eq!(
            control(
                "tiers.think.model",
                &SettingKind::Text,
                &serde_json::json!("m"),
                &models
            ),
            SettingControl::Field { text: "m".into() }
        );
    }

    #[test]
    fn a_slider_value_is_rounded_for_an_integer_key() {
        let integer = SettingKind::Integer {
            min: None,
            max: None,
        };
        let number = SettingKind::Number {
            min: None,
            max: None,
        };
        let text = |value: &str| SettingInput::Text {
            value: value.into(),
        };
        assert_eq!(
            typed(&integer, SettingInput::Number { value: 2.5 }),
            Some(serde_json::json!(3))
        );
        assert_eq!(typed(&integer, text(" 7 ")), Some(serde_json::json!(7)));
        assert_eq!(typed(&number, text(" 7.5 ")), Some(serde_json::json!(7.5)));
        assert_eq!(
            typed(&number, text("lots")),
            Some(serde_json::json!("lots"))
        );
        assert_eq!(typed(&number, text("inf")), Some(serde_json::json!("inf")));
        assert_eq!(
            typed(&number, SettingInput::Number { value: f64::NAN }),
            None
        );
        assert_eq!(
            typed(&SettingKind::Text, text(" solid ")),
            Some(serde_json::json!(" solid "))
        );
    }

    #[test]
    fn a_key_for_an_unknown_provider_is_refused() {
        let providers = ["anthropic".to_owned()];
        assert_eq!(
            check_key(&providers, "nope", "k"),
            Err(KeyError::UnknownProvider {
                provider: "nope".into()
            })
        );
        assert_eq!(
            check_key(&providers, "anthropic", " \n"),
            Err(KeyError::Empty)
        );
        assert_eq!(
            check_key(&providers, "anthropic", "  sk-test \n").as_deref(),
            Ok("sk-test")
        );
    }
}
