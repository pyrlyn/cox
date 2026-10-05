// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `ModelCaller for Session` (T33.15, PL§7d): the router, the budget gate
//! and the ledger a plugin's `cox_model_call` runs through. `cox-plugin`
//! may not depend on this crate (AGENTS.md's trust-boundary rule), so it
//! reaches this over the `ModelCaller` trait `cox-protocol` defines;
//! `crates/cox-plugin/src/hostfn.rs` blocks a plugin's worker thread on it
//! rather than `.await`ing, since that thread is a plain OS thread, not a
//! tokio runtime worker (`cox-plugin::host`).
//!
//! Shaped like `compact.rs`'s `summarise`: one request, no transcript, one
//! ledger row. What is specific to a plugin call: the job tag
//! (`Job::Plugin(id)`), and that its tier is already resolved by the
//! caller (grant-clamped, never `think`) instead of looked up in `[jobs]`
//! (`router.rs`'s `Job::Plugin` arm passes it straight through).
//!
//! `ToolInvoker for Session` (T33.13, PL§4) is here for the same reason:
//! a plugin's `cox_invoke_tool` reaches the session's tool path over a
//! trait. It adds no check of its own; `turn::run_tools` is the model's
//! path, so the engine stays the one place a call is allowed.

use async_trait::async_trait;
use cox_protocol::errors::CoreError;
use cox_protocol::ids::{CallId, TurnId};
use cox_protocol::traits::{ModelCaller, ToolInvoker};
use cox_protocol::types::{Event, Job, Level, ProviderEvent, Request, Tier, ToolResult};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::budget;
use crate::router::{Overrides, Router};
use crate::session::{Session, State};
use crate::turn::{ORIGIN, run_tools};

#[async_trait]
impl ModelCaller for Session {
    async fn call(
        &self,
        id: &str,
        tier: Tier,
        request: Request,
    ) -> Result<Vec<ProviderEvent>, CoreError> {
        // Defense in depth (D5): `hostfn.rs` already clamps to the grant
        // before calling here (`ModelTier` cannot even express `think`),
        // but this trait is the one seam every path back to the router
        // goes through, so it refuses `think` itself rather than trusting
        // every future caller to have clamped correctly.
        if tier == Tier::Think {
            return Err(CoreError::Denied {
                why: "a plugin model call may not reach the think tier".into(),
            });
        }
        let job = Job::Plugin(id.to_string());
        let route = Router::pick(&self.config, job.clone(), tier, &Overrides::default(), true)
            .map_err(|error| CoreError::Config {
                key: "tiers".into(),
                message: error.notice(),
            })?;
        // The budget gate every provider call passes (D6h/invariant 8).
        // `already_warned = true` collapses `budget::decide`'s three
        // outcomes to `Stop`/`Proceed`: a plugin call has no session-level
        // "warned once" flag to flip, and `Warn` would otherwise behave
        // exactly like `Proceed` here anyway.
        let spent = self.spent().await;
        let cap = self.config.budget.session_usd;
        if matches!(
            budget::decide(spent, cap, self.config.budget.warn_at, true),
            budget::Decision::Stop
        ) {
            return Err(CoreError::Budget { spent, cap });
        }
        // The host sets tier, model and job (PL§7d's `ModelCall` doc);
        // everything else in the plugin's own request rides unchanged.
        let req = Request {
            tier: route.tier,
            job: job.clone(),
            model: route.model.clone(),
            ..request
        };
        let (tx, mut rx) = mpsc::channel(64);
        let provider = self.provider.clone();
        let cancel = self.cancel_token();
        let join = tokio::spawn(async move { provider.stream(req, tx, cancel).await });
        let mut events = Vec::new();
        while let Some(ev) = rx.recv().await {
            events.push(ev);
        }
        let usage = join
            .await
            .map_err(|_| CoreError::Interrupted)?
            .map_err(|error| CoreError::Provider { error })?;
        let effort = route.effort;
        self.store
            .usage_insert(&cox_protocol::UsageRow {
                session_id: self.id,
                turn: 0,
                job,
                tier: route.tier,
                provider: self.provider.id(),
                model: route.model,
                effort: Some(effort),
                usage,
            })
            .map_err(|error| CoreError::Store { error })?;
        if budget::counts(route.tier, self.config.budget.cheap_counts) {
            self.add_spend(usage.cost_usd).await;
        }
        Ok(events)
    }
}

#[async_trait]
impl ToolInvoker for Session {
    async fn invoke(&self, id: &str, name: &str, input: Value) -> Result<ToolResult, CoreError> {
        // As for `!` (`user_shell`): with no turn running, an earlier `Esc`
        // may have left the token cancelled, which would deny as interrupted.
        if self.inner.lock().await.state == State::Idle {
            self.renew_cancel();
        }
        // `ToolCallRequested` carries no origin, so the transcript and the
        // rollout say whose call follows. `name` passed the grant check,
        // which only holds manifest-validated tool names.
        let origin = format!("plugin {id}");
        self.emit(Event::Notice {
            level: Level::Info,
            text: format!("{origin} runs {name}"),
        })
        .await?;
        let call = vec![(CallId::new(), name.to_string(), input)];
        let mut results = ORIGIN
            .scope(origin, run_tools(self, TurnId::new(), call))
            .await?;
        results.pop().map(|(_, r)| r).ok_or(CoreError::Interrupted)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use cox_protocol::types::{Effort, ModelId, SystemBlock, Thinking};
    use cox_provider::scripted::Scripted;

    use super::*;
    use crate::session::MemoryStore;

    fn plugin_request() -> Request {
        Request {
            tier: Tier::Cheap,
            job: Job::Main,
            model: ModelId(String::new()),
            system: vec![SystemBlock {
                text: "you are a plugin's own prompt".into(),
                cache: false,
            }],
            tools: vec![],
            messages: vec![],
            effort: Effort::Low,
            max_tokens: 256,
            thinking: Thinking::Off,
            cache_breakpoints: vec![],
            stop_sequences: vec![],
        }
    }

    fn session_with(scenario: &str, cap: f64) -> (Session, Arc<MemoryStore>) {
        let mut config = cox_protocol::Config::default();
        config.budget.session_usd = cap;
        let provider = Arc::new(Scripted::from_toml(scenario, "").expect("scenario parses"));
        let store = Arc::new(MemoryStore::new());
        let session = Session::new(
            config,
            provider,
            vec![],
            store.clone(),
            store.clone(),
            PathBuf::from("/tmp"),
        )
        .expect("session");
        (session, store)
    }

    #[tokio::test]
    async fn plugin_model_call_writes_usage_row() {
        let (session, store) = session_with("[[turn]]\ntext = \"hi from the plugin call\"\n", 5.0);
        let events = ModelCaller::call(&session, "git-glance", Tier::Cheap, plugin_request())
            .await
            .expect("plugin call succeeds");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProviderEvent::TextDelta { .. })),
            "{events:?}"
        );
        let rows = store.usage_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].job, Job::Plugin("git-glance".into()));
        assert_eq!(rows[0].tier, Tier::Cheap);
    }

    #[tokio::test]
    async fn plugin_model_call_blocked_by_budget() {
        let (session, store) = session_with("[[turn]]\ntext = \"should never be sent\"\n", 0.0);
        let error = ModelCaller::call(&session, "git-glance", Tier::Cheap, plugin_request())
            .await
            .expect_err("a zero cap refuses before the provider runs");
        assert!(matches!(error, CoreError::Budget { .. }), "{error:?}");
        assert!(store.usage_rows().is_empty());
    }

    #[tokio::test]
    async fn plugin_cannot_reach_think_tier() {
        let (session, store) = session_with("[[turn]]\ntext = \"never sent\"\n", 5.0);
        let error = ModelCaller::call(&session, "git-glance", Tier::Think, plugin_request())
            .await
            .expect_err("think is refused before it routes anywhere");
        assert!(matches!(error, CoreError::Denied { .. }), "{error:?}");
        assert!(store.usage_rows().is_empty());
    }
}
