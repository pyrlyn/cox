// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Merges a granted plugin's declarative `[[provider]]` rows
//! (`docs/design/plugins.md` §7a, `api = "chat" | "responses"`) into
//! `providers.custom`, so `crates/cox` drives them with its own
//! OpenAI-shaped wire clients — there is no wasm on the request path for
//! this form. Pure, like `cox_models::catalog`'s plugin layer (T33.16): the
//! caller (`crates/cox`) reads the granted plugins and the live
//! `ProvidersConfig`, this module only decides the merge. The ABI form
//! (`api = "plugin"`) is T33.18's `PluginProvider` and never reaches this
//! module — [`merge`] skips those rows without a warning, since they are
//! simply not this module's job.

use std::collections::HashMap;

use cox_protocol::config::{CompatibleProviderConfig, ProvidersConfig};
use cox_protocol::plugin::{ProviderApi, ProviderAuth, ProviderDecl};

/// One granted plugin's `[[provider]]` rows, as [`merge`] takes them.
/// Borrowed from the manifest, like `cox_models::catalog::PluginModels`
/// (T33.16), so this crate needs no store or plugin host to decide the
/// merge.
#[derive(Debug, Clone, Copy)]
pub struct PluginProviders<'a> {
    /// The plugin id: the tie-break between two plugins claiming the same
    /// new section name, and named in every skipped-section warning.
    pub plugin: &'a str,
    /// Its `[[provider]]` rows.
    pub decls: &'a [ProviderDecl],
}

/// [`merge`]'s result.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Merged {
    /// New `providers.custom` entries the caller should add to the live
    /// config. A section name a config (or a native `[providers.*]` table)
    /// already uses is never in here.
    pub custom: HashMap<String, CompatibleProviderConfig>,
    /// One line per `[[provider]]` row that was skipped, for
    /// `Notice(Warn)` — fail open on extensions (AGENTS.md "Trust
    /// boundaries").
    pub warnings: Vec<String>,
}

/// `ProvidersConfig`'s own named sections: each carries knobs
/// (`cache_ttl`, `fallbacks`, `load`, …) a `CompatibleProviderConfig`
/// cannot express, so a plugin can never take one of these names — only
/// `providers.custom`'s flattened map, which is exactly what [`merge`]
/// fills.
const RESERVED: &[&str] = &["anthropic", "openai", "local", "lmstudio", "typesafe"];

/// Builds the plugin layer of `providers.custom` (PL§7a, T33.17): each
/// granted plugin's `api = "chat" | "responses"` `[[provider]]` row becomes
/// one more compatible section, unless a user config section of the same
/// name already exists (native or `custom`) — that wins, and the plugin's
/// section is skipped with a notice, exactly as the design doc says. A
/// name two plugins both claim goes to the lower plugin id, so the order
/// `plugins` is given in never decides the winner — the same rule
/// `cox_models::catalog::PluginModels` uses for a contested model id
/// (T33.16).
pub fn merge(providers: &ProvidersConfig, plugins: &[PluginProviders<'_>]) -> Merged {
    let mut ordered: Vec<&PluginProviders<'_>> = plugins.iter().collect();
    ordered.sort_by_key(|p| p.plugin);
    let mut out = Merged::default();
    let mut owners: HashMap<String, String> = HashMap::new();
    for p in ordered {
        for decl in p.decls {
            merge_one(providers, p.plugin, decl, &mut out, &mut owners);
        }
    }
    out
}

fn merge_one(
    providers: &ProvidersConfig,
    plugin: &str,
    decl: &ProviderDecl,
    out: &mut Merged,
    owners: &mut HashMap<String, String>,
) {
    let api = match decl.api {
        ProviderApi::Chat => "chat",
        ProviderApi::Responses => "responses",
        // The ABI form has no wire client here; T33.18's `PluginProvider`
        // drives `api = "plugin"` outside `providers.custom`.
        ProviderApi::Plugin => return,
    };
    if RESERVED.contains(&decl.name.as_str()) || providers.custom.contains_key(&decl.name) {
        out.warnings.push(format!(
            "plugin {plugin}'s provider section `{}` is shadowed by an existing `providers.{}` config section",
            decl.name, decl.name
        ));
        return;
    }
    if let Some(owner) = owners.get(&decl.name) {
        out.warnings.push(format!(
            "plugin {plugin} also declares provider {}; plugin {owner}'s section is kept",
            decl.name
        ));
        return;
    }
    if decl.auth == ProviderAuth::XApiKey {
        // The declarative chat/responses wire only ever sends
        // `Authorization: Bearer <key>` (`crates/cox-provider-openai`); a
        // section that needs `x-api-key` must use the ABI form (T33.18)
        // instead of silently getting the wrong header.
        out.warnings.push(format!(
            "plugin {plugin}'s provider section `{}` asks for x-api-key auth, which the declarative chat/responses client cannot send; skipped",
            decl.name
        ));
        return;
    }
    owners.insert(decl.name.clone(), plugin.to_string());
    out.custom.insert(
        decl.name.clone(),
        CompatibleProviderConfig {
            base_url: decl.base_url.clone(),
            api_key_env: decl.api_key_env.clone().unwrap_or_default(),
            api: api.to_string(),
            ..CompatibleProviderConfig::default()
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(name: &str, api: ProviderApi) -> ProviderDecl {
        ProviderDecl {
            name: name.into(),
            api,
            base_url: "https://api.example.test".into(),
            api_key_env: Some("EXAMPLE_API_KEY".into()),
            auth: ProviderAuth::Bearer,
        }
    }

    #[test]
    fn new_section_merges_as_compatible_config() {
        let decls = [decl("acme", ProviderApi::Chat)];
        let plugins = [PluginProviders {
            plugin: "acme-pkg",
            decls: &decls,
        }];
        let merged = merge(&ProvidersConfig::default(), &plugins);
        assert!(merged.warnings.is_empty(), "{:?}", merged.warnings);
        let section = merged.custom.get("acme").expect("merged section");
        assert_eq!(section.base_url, "https://api.example.test");
        assert_eq!(section.api_key_env, "EXAMPLE_API_KEY");
        assert_eq!(section.api, "chat");
    }

    #[test]
    fn config_section_shadows_plugin_section() {
        let decls = [decl("acme", ProviderApi::Chat)];
        let plugins = [PluginProviders {
            plugin: "acme-pkg",
            decls: &decls,
        }];
        let mut providers = ProvidersConfig::default();
        providers
            .custom
            .insert("acme".into(), CompatibleProviderConfig::default());
        let merged = merge(&providers, &plugins);
        assert!(
            merged.custom.is_empty(),
            "the plugin section is skipped, not merged"
        );
        assert_eq!(
            merged.warnings,
            [
                "plugin acme-pkg's provider section `acme` is shadowed by an existing `providers.acme` config section"
            ]
        );
    }

    #[test]
    fn reserved_native_names_are_never_taken() {
        let decls = [decl("anthropic", ProviderApi::Chat)];
        let plugins = [PluginProviders {
            plugin: "acme-pkg",
            decls: &decls,
        }];
        let merged = merge(&ProvidersConfig::default(), &plugins);
        assert!(merged.custom.is_empty());
        assert_eq!(merged.warnings.len(), 1, "{:?}", merged.warnings);
    }

    #[test]
    fn duplicate_plugin_names_lower_id_wins() {
        let alpha_decls = [decl("shared", ProviderApi::Chat)];
        let zeta_decls = [ProviderDecl {
            base_url: "https://other.test".into(),
            ..decl("shared", ProviderApi::Chat)
        }];
        let plugins = [
            PluginProviders {
                plugin: "zeta",
                decls: &zeta_decls,
            },
            PluginProviders {
                plugin: "alpha",
                decls: &alpha_decls,
            },
        ];
        let merged = merge(&ProvidersConfig::default(), &plugins);
        let section = merged.custom.get("shared").expect("kept section");
        assert_eq!(
            section.base_url, "https://api.example.test",
            "alpha's row, not zeta's"
        );
        assert_eq!(
            merged.warnings,
            ["plugin zeta also declares provider shared; plugin alpha's section is kept"]
        );
    }

    #[test]
    fn plugin_api_form_is_left_to_t33_18() {
        let decls = [decl("typesafe-guest", ProviderApi::Plugin)];
        let plugins = [PluginProviders {
            plugin: "jev",
            decls: &decls,
        }];
        let merged = merge(&ProvidersConfig::default(), &plugins);
        assert!(merged.custom.is_empty());
        assert!(
            merged.warnings.is_empty(),
            "silently out of scope, not an error"
        );
    }

    #[test]
    fn x_api_key_auth_is_unsupported_declaratively() {
        let decls = [ProviderDecl {
            auth: ProviderAuth::XApiKey,
            ..decl("acme", ProviderApi::Chat)
        }];
        let plugins = [PluginProviders {
            plugin: "acme-pkg",
            decls: &decls,
        }];
        let merged = merge(&ProvidersConfig::default(), &plugins);
        assert!(merged.custom.is_empty());
        assert_eq!(merged.warnings.len(), 1);
        assert!(
            merged.warnings[0].contains("x-api-key"),
            "{:?}",
            merged.warnings
        );
    }

    #[test]
    fn missing_api_key_env_builds_a_keyless_section() {
        let decls = [ProviderDecl {
            api_key_env: None,
            ..decl("acme", ProviderApi::Responses)
        }];
        let plugins = [PluginProviders {
            plugin: "acme-pkg",
            decls: &decls,
        }];
        let merged = merge(&ProvidersConfig::default(), &plugins);
        let section = merged.custom.get("acme").expect("merged section");
        assert_eq!(section.api_key_env, "");
        assert_eq!(section.api, "responses");
    }
}
