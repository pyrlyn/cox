// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! One plugin as a decision-point source (PL§4 "Decision points", T33.20):
//! `PluginAdvisor` implements `cox_protocol::traits::Advisor` by calling the
//! guest's `cox_decide` export. Shaped like `hooks.rs`, and here for the same
//! reason: this is the only crate that may hold a `PluginHost`. The core
//! keeps the decision; this only carries the question and the answer.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cox_plugin_api::{Advice, Question};
use cox_protocol::traits::Advisor;

use crate::host::{Lane, PluginHost};

const EXPORT: &str = "cox_decide";
/// The granted-capability line prefix `grant::capability_list` writes.
const GRANT_PREFIX: &str = "decide:";

/// A loaded plugin's granted decision points. The deadline is the point's
/// latency budget, clamped by `PluginHost::call` to the plugin's
/// `limits.call_ms`.
pub struct PluginAdvisor {
    id: String,
    host: Arc<PluginHost>,
    points: BTreeSet<String>,
}

impl PluginAdvisor {
    /// `granted` is the plugin's granted-capability list; only its
    /// `decide:<point>` lines count.
    pub fn new(id: impl Into<String>, host: Arc<PluginHost>, granted: &[String]) -> Self {
        let points = granted
            .iter()
            .filter_map(|line| line.strip_prefix(GRANT_PREFIX))
            .map(str::to_owned)
            .collect();
        Self {
            id: id.into(),
            host,
            points,
        }
    }
}

#[async_trait]
impl Advisor for PluginAdvisor {
    fn id(&self) -> &str {
        &self.id
    }

    /// An ungranted point never reaches the guest. A trap, a timeout, a
    /// missing export or output that is not an `Option<Advice>` is silence,
    /// logged and skipped (D14): the core then keeps its static pick.
    async fn advise(&self, question: Question, budget: Duration) -> Option<Advice> {
        let point = serde_json::to_value(question.point).ok()?;
        if !point.as_str().is_some_and(|p| self.points.contains(p)) {
            return None;
        }
        let host = self.host.clone();
        // `PluginHost::call` blocks until the plugin's worker answers, so it
        // must not hold a runtime thread.
        let answer = tokio::task::spawn_blocking(move || {
            host.call::<_, Option<Advice>>(Lane::Control, EXPORT, &question, budget)
        })
        .await;
        let why = match answer {
            Ok(Ok(Some(advice))) => return advice,
            Ok(Ok(None)) => format!("grants {point} but has no {EXPORT} export"),
            Ok(Err(e)) => e.to_string(),
            Err(e) => e.to_string(),
        };
        tracing::warn!(plugin = %self.id, %point, "decision skipped: {why}");
        None
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use cox_plugin_api::{Answer, DecidePoint, Limits};
    use serde_json::json;

    use super::*;
    use crate::host::tests::{answering, module, spinning};

    fn advisor(body: &str, limits: &Limits, points: &[&str]) -> PluginAdvisor {
        let host = PluginHost::load("p", &module(body), limits).expect("module loads");
        let granted: Vec<String> = points
            .iter()
            .map(|p| format!("{GRANT_PREFIX}{p}"))
            .collect();
        PluginAdvisor::new("p", Arc::new(host), &granted)
    }

    fn route_question() -> Question {
        Question {
            point: DecidePoint::Route,
            state: json!({ "prompt": "rename foo" }),
            options: vec!["cheap".into(), "code".into()],
        }
    }

    const ADVICE: &str = r#"{"answer":{"kind":"choice","order":[0]},"confidence":0.9}"#;

    #[tokio::test]
    async fn plugin_advisor_answers_only_granted_points() {
        let granted = advisor(&answering(EXPORT, ADVICE), &Limits::default(), &["route"]);
        let advice = granted
            .advise(route_question(), Duration::from_secs(5))
            .await
            .expect("a granted point is answered");
        assert_eq!(advice.answer, Answer::Choice { order: vec![0] });
        assert_eq!(advice.confidence, Some(0.9));
        let ungranted = advisor(&answering(EXPORT, ADVICE), &Limits::default(), &["risk"]);
        assert_eq!(
            ungranted
                .advise(route_question(), Duration::from_secs(5))
                .await,
            None
        );
    }

    #[tokio::test]
    async fn plugin_advisor_is_silent_past_its_budget() {
        let limits = Limits {
            call_ms: Some(30_000),
            ..Limits::default()
        };
        let spin = advisor(&spinning(EXPORT), &limits, &["route"]);
        let started = Instant::now();
        let advice = spin
            .advise(route_question(), Duration::from_millis(100))
            .await;
        assert_eq!(advice, None);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn plugin_advisor_without_export_is_silent() {
        let none = advisor("", &Limits::default(), &["route"]);
        assert_eq!(
            none.advise(route_question(), Duration::from_secs(5)).await,
            None
        );
    }
}
