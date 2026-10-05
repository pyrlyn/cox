// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! One plugin as a hook source (PL§6, T33.11): `PluginHooks` implements
//! `cox_protocol::Hook` by calling the guest's `cox_hook` export, so plugin
//! hooks are one more source in `cox-ext`'s `HookChain` rather than a second
//! mechanism. Here, not in `cox-ext`, because it is the only place that may
//! hold a `PluginHost`.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cox_plugin_api::HookCall;
use cox_protocol::config::HooksConfig;
use cox_protocol::traits::Hook;
use cox_protocol::types::{HookEvent, HookOutcome};
use serde_json::Value;

use crate::host::{Lane, PluginHost};

const EXPORT: &str = "cox_hook";
/// The granted-capability line prefix `grant::capability_list` writes.
const GRANT_PREFIX: &str = "hooks:";

/// A loaded plugin's granted hooks. The deadline is the smaller of the
/// core's `hooks.timeout_s` and the plugin's `limits.call_ms`, because
/// `PluginHost::call` clamps every deadline to the latter.
pub struct PluginHooks {
    id: String,
    host: Arc<PluginHost>,
    events: BTreeSet<String>,
}

impl PluginHooks {
    /// `granted` is the plugin's granted-capability list (the one
    /// `HostEnv::with_grant` gets); only its `hooks:<HookEvent>` lines count.
    pub fn new(id: impl Into<String>, host: Arc<PluginHost>, granted: &[String]) -> Self {
        let events = granted
            .iter()
            .filter_map(|line| line.strip_prefix(GRANT_PREFIX))
            .map(str::to_owned)
            .collect();
        Self {
            id: id.into(),
            host,
            events,
        }
    }

    /// The plugin id, which orders plugin hooks in the chain.
    pub fn id(&self) -> &str {
        &self.id
    }

    fn failed(&self, why: impl std::fmt::Display) -> HookOutcome {
        HookOutcome::Failed {
            error: format!("plugin {}: {why}", self.id),
        }
    }
}

#[async_trait]
impl Hook for PluginHooks {
    fn interested(&self, event: HookEvent, _config: &HooksConfig) -> bool {
        self.events.contains(event.name())
    }

    /// An ungranted event never reaches the guest. Every error — a trap, a
    /// timeout, a missing export, output that is not a `HookOutcome` — is
    /// `Failed`, which the core turns into a warning and skips (D14).
    async fn run(&self, event: HookEvent, payload: Value, timeout: Duration) -> HookOutcome {
        if !self.events.contains(event.name()) {
            return HookOutcome::Continue;
        }
        // `HookCall.event` is the protocol's own `HookEvent` form (ABI §4).
        let call = match serde_json::to_value(event) {
            Ok(event) => HookCall { event, payload },
            Err(e) => return self.failed(e),
        };
        let host = self.host.clone();
        // `PluginHost::call` blocks until the plugin's worker answers, so it
        // must not hold a runtime thread.
        let answer = tokio::task::spawn_blocking(move || {
            host.call::<_, HookOutcome>(Lane::Control, EXPORT, &call, timeout)
        })
        .await;
        match answer {
            Ok(Ok(Some(outcome))) => outcome,
            Ok(Ok(None)) => self.failed(format!("grants hooks but has no {EXPORT} export")),
            Ok(Err(e)) => self.failed(e),
            Err(e) => self.failed(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use cox_plugin_api::Limits;
    use serde_json::json;

    use super::*;
    use crate::host::tests::{answering, module, spinning};

    fn granted(events: &[&str]) -> Vec<String> {
        events
            .iter()
            .map(|e| format!("{GRANT_PREFIX}{e}"))
            .collect()
    }

    fn plugin(body: &str, limits: &Limits, events: &[&str]) -> PluginHooks {
        let host = PluginHost::load("p", &module(body), limits).expect("module loads");
        PluginHooks::new("p", Arc::new(host), &granted(events))
    }

    #[tokio::test]
    async fn plugin_hook_answers_only_its_granted_events() {
        let hooks = plugin(
            &answering(EXPORT, r#"{"type":"block","reason":"no"}"#),
            &Limits::default(),
            &["PreToolUse"],
        );
        let config = HooksConfig::default();
        assert!(hooks.interested(HookEvent::PreToolUse, &config));
        assert!(!hooks.interested(HookEvent::Stop, &config));
        let pre = hooks
            .run(HookEvent::PreToolUse, json!({}), Duration::from_secs(5))
            .await;
        assert_eq!(
            pre,
            HookOutcome::Block {
                reason: "no".into()
            }
        );
        let stop = hooks
            .run(HookEvent::Stop, json!({}), Duration::from_secs(5))
            .await;
        assert_eq!(stop, HookOutcome::Continue);
    }

    /// The core turns this `Failed` into `Notice(Warn)` and continues
    /// (`broken_hook_is_skipped_not_fatal`); here, both deadlines bound it.
    #[tokio::test]
    async fn plugin_hook_timeout_fails_open_with_notice() {
        let short_call_ms = Limits {
            call_ms: Some(100),
            ..Limits::default()
        };
        let long_call_ms = Limits {
            call_ms: Some(30_000),
            ..Limits::default()
        };
        for (limits, timeout) in [
            (short_call_ms, Duration::from_secs(60)),
            (long_call_ms, Duration::from_millis(100)),
        ] {
            let hooks = plugin(&spinning(EXPORT), &limits, &["PreToolUse"]);
            let started = Instant::now();
            let out = hooks.run(HookEvent::PreToolUse, json!({}), timeout).await;
            assert!(
                matches!(&out, HookOutcome::Failed { error } if error.starts_with("plugin p:")),
                "{out:?}"
            );
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "{:?}",
                started.elapsed()
            );
        }
    }
}
