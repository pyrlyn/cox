// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Fail-open for a tier whose provider became a plugin (T33.40.13, D14). A
//! config written when Jev was built in names `typesafe`; without the `jev`
//! plugin loaded, that name has no client, and `open` would fail on it. It
//! lives here, not in `cox-core`'s router, because it needs the loaded
//! plugin set and the embedded defaults, and the router stays a pure
//! function of an already-settled `Config`.

use std::collections::HashSet;

use cox_protocol::Config;
use cox_protocol::config::{TierConfig, TiersConfig};

/// Provider sections that were built in and now belong to a plugin: the
/// section name, the plugin id and its display name.
const MOVED: &[(&str, &str, &str)] = &[("typesafe", "jev", "Jev")];

/// Moves every tier naming a moved section that no loaded plugin serves onto
/// the default tier's provider and model; the tier's effort, thinking and
/// limits stay the user's. Returns one notice per section moved.
pub(crate) fn fall_back(
    config: &mut Config,
    loaded: &HashSet<String>,
    defaults: &TiersConfig,
) -> Vec<String> {
    let mut notices = Vec::new();
    for &(section, plugin, name) in MOVED {
        if loaded.contains(section) {
            continue;
        }
        let mut moved = false;
        for (tier, default) in [
            (&mut config.tiers.cheap, &defaults.cheap),
            (&mut config.tiers.code, &defaults.code),
            (&mut config.tiers.think, &defaults.think),
        ] {
            moved |= take_default(tier, default, section);
        }
        if moved {
            notices.push(format!(
                "{name} moved to a plugin: build `plugins/{plugin}` and run \
                 `cox plugin install <dir>`"
            ));
        }
    }
    notices
}

fn take_default(tier: &mut TierConfig, default: &TierConfig, section: &str) -> bool {
    if tier.provider != section || default.provider == section {
        return false;
    }
    tier.provider.clone_from(&default.provider);
    tier.model.clone_from(&default.model);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> TiersConfig {
        cox_config::load::default_tiers().expect("embedded defaults")
    }

    fn legacy() -> Config {
        let mut config = Config::default();
        config.tiers.code.provider = "typesafe".into();
        config.tiers.code.model = "jev-latest".into();
        config.tiers.code.max_tokens = 123;
        config
    }

    #[test]
    fn legacy_typesafe_tier_without_plugin_uses_default_tier_with_notice() {
        let mut config = legacy();
        let notices = fall_back(&mut config, &HashSet::new(), &defaults());
        assert_eq!(config.tiers.code.provider, defaults().code.provider);
        assert_eq!(config.tiers.code.model, defaults().code.model);
        assert_eq!(config.tiers.code.max_tokens, 123, "the user's limits stay");
        assert_eq!(
            notices,
            ["Jev moved to a plugin: build `plugins/jev` and run `cox plugin install <dir>`"]
        );
    }

    #[test]
    fn legacy_typesafe_tier_with_plugin_is_left_alone() {
        let mut config = legacy();
        let loaded = HashSet::from(["typesafe".to_string()]);
        let notices = fall_back(&mut config, &loaded, &defaults());
        assert_eq!(config, legacy());
        assert!(notices.is_empty());
    }

    #[test]
    fn config_without_a_legacy_tier_gets_no_notice() {
        let mut config = Config::default();
        assert!(fall_back(&mut config, &HashSet::new(), &defaults()).is_empty());
        assert_eq!(config, Config::default());
    }
}
