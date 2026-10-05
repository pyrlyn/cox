// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The fold behind `TimelinePatch::Status` (T37.24.7): the composer's mode,
//! model and effort chips, and the queue count. Seeded from the config the
//! session opened with — the core records its opening mode to the rollout
//! only (T50.4), because every surface already has it from that config —
//! then kept by `StateChanged`, `TurnStarted` and `ModelSwitched`; the
//! model's display name comes from the model catalog (A111). Separate
//! from `Timeline`, whose fold is the block list, and from `Controller`,
//! which only queues what this says changed.

use std::collections::HashMap;

use cox_core::permission::next_mode;
use cox_protocol::Config;
use cox_protocol::types::{Effort, Event, Job, ModelId, PermissionMode, Tier};

use crate::patch::Status;

/// One catalog name plus the shortened form the toolbar chip shows (A111,
/// A116). A new record, not a change to `Status.model_name`: the TUI and
/// ACP still read the full name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelName {
    pub name: String,
    pub short_name: String,
}

/// Model id → catalog name for every row that has one (A111). `.get` still
/// yields the full name so `ModelChoice.display_name` and the status chip
/// keep their current meaning (A129).
#[derive(Debug, Clone, Default)]
pub(crate) struct ModelNames {
    by_id: HashMap<String, ModelName>,
}

impl ModelNames {
    /// The catalog's full name; `None` when the catalog has none.
    pub(crate) fn get(&self, id: impl AsRef<str>) -> Option<&String> {
        self.by_id.get(id.as_ref()).map(|n| &n.name)
    }

    /// The chip's shorter form of `get`; `None` when the catalog has no name.
    pub(crate) fn short(&self, id: impl AsRef<str>) -> Option<&String> {
        self.by_id.get(id.as_ref()).map(|n| &n.short_name)
    }
}

/// Model id → display name for every catalog row that has one (A111). The
/// built-in rows are under `config`'s, so an entry that names nothing keeps
/// models.dev's name; a catalog that fails to load names nothing, and the
/// app shows the id.
pub(crate) fn model_names(config: &Config) -> ModelNames {
    cox_models::Catalog::load(config, &[], None)
        .map(|catalog| ModelNames {
            by_id: catalog
                .rows()
                .filter_map(|r| {
                    let name = r.display_name.clone()?;
                    let short_name = shorten(&name);
                    Some((r.id.clone(), ModelName { name, short_name }))
                })
                .collect(),
        })
        .unwrap_or_default()
}

/// Drops a leading `Claude ` and a trailing ` (latest)` so the chip can
/// fit; a name that is only the prefix or suffix is left whole rather
/// than showing nothing.
pub(crate) fn shorten(name: &str) -> String {
    let name = match name.strip_suffix(" (latest)") {
        Some(rest) if !rest.is_empty() => rest,
        _ => name,
    };
    match name.strip_prefix("Claude ") {
        Some(rest) if !rest.is_empty() => rest.to_owned(),
        _ => name.to_owned(),
    }
}

/// The status and what it needs to put back when an override is cleared.
#[derive(Debug, Clone, Default)]
pub struct StatusFold {
    status: Status,
    /// The `code` tier's configured effort, which `SetEffort { None }`
    /// restores.
    tier_effort: Option<Effort>,
    /// Model id → catalog name, for every catalog row that has one.
    names: ModelNames,
}

impl StatusFold {
    /// The status a session opened with `config` starts in.
    pub fn open(config: &Config) -> Self {
        let code = &config.tiers.code;
        let names = model_names(config);
        let mut fold = Self {
            status: Status::default(),
            tier_effort: Some(code.effort),
            names,
        };
        fold.name(ModelId(code.model.clone()));
        fold.set(config.permissions.mode, None);
        fold
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Changes the queue count.
    pub fn queue(&mut self, change: impl FnOnce(&mut u32)) {
        change(&mut self.status.queued);
    }

    /// Folds `event` in; says whether the status changed.
    pub fn apply(&mut self, event: &Event) -> bool {
        let before = self.status.clone();
        match event {
            Event::StateChanged { mode, effort } => self.set(*mode, *effort),
            // A subagent's or compaction's turn is not the one the chip names.
            Event::TurnStarted {
                job: Job::Main,
                model,
                ..
            } => self.name(model.clone()),
            Event::ModelSwitched {
                tier: Tier::Code,
                to,
                ..
            } => self.name(to.clone()),
            _ => {}
        }
        self.status != before
    }

    fn name(&mut self, model: ModelId) {
        self.status.model_name = self.names.get(&model.0).cloned();
        self.status.short_name = self.names.short(&model.0).cloned();
        self.status.model = Some(model);
    }

    fn set(&mut self, mode: PermissionMode, effort: Option<Effort>) {
        self.status.mode = Some(mode);
        self.status.next_mode = Some(next_mode(mode));
        self.status.effort = effort.or(self.tier_effort);
    }
}

#[cfg(test)]
mod tests {
    use cox_protocol::ids::TurnId;

    use super::*;

    fn opened() -> StatusFold {
        let mut config = Config::default();
        config.tiers.code.model = "claude-sonnet-5".into();
        config.tiers.code.effort = Effort::High;
        config.permissions.mode = PermissionMode::Plan;
        StatusFold::open(&config)
    }

    fn turn(job: Job, model: &str) -> Event {
        Event::TurnStarted {
            turn: TurnId::new(),
            seq: 1,
            job,
            tier: Tier::Code,
            model: ModelId(model.into()),
        }
    }

    #[test]
    fn a_session_opens_with_the_configured_mode_model_and_effort() {
        let status = opened().status().clone();
        assert_eq!(status.mode, Some(PermissionMode::Plan));
        assert_eq!(status.next_mode, Some(PermissionMode::Auto));
        assert_eq!(status.model, Some(ModelId("claude-sonnet-5".into())));
        assert_eq!(status.effort, Some(Effort::High));
    }

    #[test]
    fn state_changed_sets_the_mode_and_a_cleared_effort_falls_back_to_the_tier() {
        let mut fold = opened();
        let changed = |mode, effort| Event::StateChanged { mode, effort };
        assert!(fold.apply(&changed(PermissionMode::Auto, Some(Effort::Low))));
        assert_eq!(fold.status().mode, Some(PermissionMode::Auto));
        assert_eq!(fold.status().next_mode, Some(PermissionMode::Default));
        assert_eq!(fold.status().effort, Some(Effort::Low));
        assert!(fold.apply(&changed(PermissionMode::Auto, None)));
        assert_eq!(fold.status().effort, Some(Effort::High));
        assert!(
            !fold.apply(&changed(PermissionMode::Auto, None)),
            "no change"
        );
    }

    #[test]
    fn only_a_main_turn_or_a_code_tier_switch_names_the_model() {
        let mut fold = opened();
        assert!(!fold.apply(&turn(Job::Compact, "claude-haiku-5")));
        assert!(fold.apply(&turn(Job::Main, "claude-opus-5")));
        assert_eq!(fold.status().model, Some(ModelId("claude-opus-5".into())));
        let switch = |tier| Event::ModelSwitched {
            tier,
            from: ModelId("claude-opus-5".into()),
            to: ModelId("gpt-6".into()),
        };
        assert!(!fold.apply(&switch(Tier::Cheap)));
        assert!(fold.apply(&switch(Tier::Code)));
        assert_eq!(fold.status().model, Some(ModelId("gpt-6".into())));
    }

    #[test]
    fn the_status_names_the_model_from_the_catalog_and_an_unknown_one_by_nothing() {
        let mut fold = opened();
        assert_eq!(fold.status().model_name.as_deref(), Some("Claude Sonnet 5"));
        assert!(fold.apply(&turn(Job::Main, "claude-opus-5")));
        assert_eq!(fold.status().model_name.as_deref(), Some("Claude Opus 5"));
        assert!(fold.apply(&turn(Job::Main, "my-local-model")));
        assert_eq!(fold.status().model_name, None);
    }

    #[test]
    fn a_short_name_drops_the_vendor_and_latest() {
        assert_eq!(shorten("Claude Haiku 4.5 (latest)"), "Haiku 4.5");
        assert_eq!(shorten("Claude Sonnet 5"), "Sonnet 5");
        assert_eq!(shorten("GPT-5.1"), "GPT-5.1");
        assert_eq!(shorten("Claude "), "Claude ");
        let mut config = Config::default();
        config.tiers.code.model = "claude-sonnet-5".into();
        config.providers.anthropic.models = vec![cox_protocol::config::ProviderModel {
            id: "claude-haiku-4-5".into(),
            display_name: Some("Claude Haiku 4.5 (latest)".into()),
            ..Default::default()
        }];
        let names = model_names(&config);
        let haiku = names.by_id.get("claude-haiku-4-5").expect("catalog row");
        assert_eq!(haiku.name, "Claude Haiku 4.5 (latest)");
        assert_eq!(haiku.short_name, "Haiku 4.5");
        let fold = StatusFold::open(&config);
        assert_eq!(
            fold.status().model_name.as_deref(),
            Some("Claude Sonnet 5"),
            "Status.model_name stays the full catalog name"
        );
        assert_eq!(fold.status().short_name.as_deref(), Some("Sonnet 5"));
    }
}
