// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! One plugin as a decision-point source (PL§4 "Decision points", T33.20):
//! `PluginAdvisor` implements `cox_protocol::traits::Advisor` by calling the
//! guest's `cox_decide` export. Shaped like `hooks.rs`, and here for the same
//! reason: this is the only crate that may hold a `PluginHost`. The core
//! keeps the decision; this only carries the question and the answer.
//!
//! A decision plugin that needs its own model answers in two phases
//! (T33.40.1, J§4.1): `cox_decide` returns `DecideOut::Call`, the host runs
//! it on the plugin's own provider section once the worker is free (so
//! `cox_provider_stream` cannot wait on the export that asked for it), and
//! `cox_decide_resume` turns the events into advice. The call goes through
//! `ModelCaller::call_own`, the session's budget gate and ledger.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cox_plugin_api::{
    Advice, CallTarget, DecideOut, DecideResume, ModelCall, NoticeLevel, OwnProvider, Question,
};
use cox_protocol::traits::{Advisor, ModelCaller};
use cox_protocol::types::Request;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::host::{Lane, PluginHost};
use crate::hostfn::HostEnv;

const EXPORT: &str = "cox_decide";
const RESUME: &str = "cox_decide_resume";
/// The granted-capability line prefix `grant::capability_list` writes.
const GRANT_PREFIX: &str = "decide:";
/// Failures in a row that disable `cox_decide` for the session (PL§10).
const STRIKES: u32 = 3;

/// A loaded plugin's granted decision points. The deadline is the point's
/// latency budget, clamped by `PluginHost::call` to the plugin's
/// `limits.call_ms`; with a call-out, the decide call, the provider call
/// and the resume share it.
pub struct PluginAdvisor {
    id: String,
    host: Arc<PluginHost>,
    points: BTreeSet<String>,
    call_out: Option<CallOut>,
    /// Failures since the last answer or silence; at [`STRIKES`] the export
    /// is never called again in this session.
    failures: AtomicU32,
}

/// What a `DecideOut::Call` runs through: the plugin's own environment,
/// which knows its bound provider sections, and the session.
struct CallOut {
    env: Arc<HostEnv>,
    caller: Arc<dyn ModelCaller>,
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
            call_out: None,
            failures: AtomicU32::new(0),
        }
    }

    /// Lets `DecideOut::Call` reach the provider sections bound in `env`
    /// through `caller`. Without it a call-out is silence.
    pub fn with_call_out(mut self, env: Arc<HostEnv>, caller: Arc<dyn ModelCaller>) -> Self {
        self.call_out = Some(CallOut { env, caller });
        self
    }

    /// Calls `export` without holding a runtime thread: `PluginHost::call`
    /// blocks until the plugin's worker answers. `Ok(None)` is a missing
    /// export.
    async fn ask<I, O>(
        &self,
        export: &'static str,
        input: I,
        budget: Duration,
    ) -> Result<Option<O>, String>
    where
        I: Serialize + Send + 'static,
        O: DeserializeOwned + Send + 'static,
    {
        let host = self.host.clone();
        tokio::task::spawn_blocking(move || {
            host.call::<_, O>(Lane::Control, export, &input, budget)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
    }

    /// Runs `call` on the plugin's own section, then `cox_decide_resume`,
    /// both inside what is left of the point's budget.
    async fn call_out(
        &self,
        question: Question,
        call: ModelCall,
        deadline: Instant,
    ) -> Result<Option<Advice>, String> {
        let CallTarget::OwnProvider {
            own_provider: OwnProvider { name, model },
        } = call.target
        else {
            return Err("a decide call-out must target the plugin's own provider".into());
        };
        let out = self
            .call_out
            .as_ref()
            .ok_or("decide call-outs are not available in this cox")?;
        if !out.env.owns_provider(&name) {
            return Err(format!("provider section `{name}` is not this plugin's"));
        }
        let request: Request =
            serde_json::from_value(call.request).map_err(|e| format!("bad request: {e}"))?;
        let left = deadline.saturating_duration_since(Instant::now());
        let events =
            tokio::time::timeout(left, out.caller.call_own(&self.id, &name, &model, request))
                .await
                .map_err(|_| "the call-out ran past the point's budget".to_string())?
                .map_err(|e| e.to_string())?;
        let events = events
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        let resume = DecideResume { question, events };
        let left = deadline.saturating_duration_since(Instant::now());
        self.ask::<_, Option<Advice>>(RESUME, resume, left)
            .await?
            .ok_or_else(|| format!("returned a call but has no {RESUME} export"))
    }
}

#[async_trait]
impl Advisor for PluginAdvisor {
    fn id(&self) -> &str {
        &self.id
    }

    /// An ungranted point never reaches the guest. A trap, a timeout, a
    /// missing export, output that is not a `DecideOut`, a failed or
    /// refused call-out, or an answer that does not fit the question is
    /// silence, logged and skipped (D14): the core then keeps its static
    /// pick.
    async fn advise(&self, question: Question, budget: Duration) -> Option<Advice> {
        let point = serde_json::to_value(question.point).ok()?;
        if !point.as_str().is_some_and(|p| self.points.contains(p))
            || self.failures.load(Ordering::Relaxed) >= STRIKES
        {
            return None;
        }
        let deadline = Instant::now() + budget;
        let answer = match self
            .ask::<_, DecideOut>(EXPORT, question.clone(), budget)
            .await
        {
            Ok(Some(DecideOut::Advice(advice))) => Ok(advice),
            Ok(Some(DecideOut::Call { call })) => {
                self.call_out(question.clone(), call, deadline).await
            }
            Ok(None) => Err(format!("grants {point} but has no {EXPORT} export")),
            Err(why) => Err(why),
        };
        let why = match answer {
            Ok(None) => {
                self.failures.store(0, Ordering::Relaxed);
                return None;
            }
            Ok(Some(advice)) if question.fits(&advice.answer) => {
                self.failures.store(0, Ordering::Relaxed);
                return Some(advice);
            }
            Ok(Some(_)) => "the answer does not fit the question".to_string(),
            Err(why) => why,
        };
        tracing::warn!(plugin = %self.id, %point, "decision skipped: {why}");
        self.strike(&why);
        None
    }
}

impl PluginAdvisor {
    /// Counts one failure. The first of a run and the one that disables the
    /// export each leave one notice, so a dead key or endpoint is visible
    /// without a notice per question (D14).
    fn strike(&self, why: &str) {
        let n = self.failures.fetch_add(1, Ordering::Relaxed) + 1;
        let text = match n {
            1 => format!("decision skipped: {why}"),
            STRIKES => {
                format!("{EXPORT} disabled for this session after {STRIKES} failures in a row")
            }
            _ => return,
        };
        if let Some(out) = &self.call_out {
            // A full notice queue only loses the notice; the decision is
            // already skipped.
            let _ = out.env.notify(NoticeLevel::Warn, &text);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use cox_plugin_api::{Answer, DecidePoint, Limits, QuestionItem};
    use cox_protocol::errors::CoreError;
    use cox_protocol::traits::Provider;
    use cox_protocol::types::{Effort, Job, ModelId, ProviderEvent, Thinking, Tier};
    use serde_json::json;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::host::tests::{answering, answering_at, module, spinning};
    use crate::provider::PluginProvider;

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
            items: vec![],
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

    #[tokio::test]
    async fn batched_question_answers_each_item() {
        let batch = || Question {
            point: DecidePoint::Risk,
            state: json!({ "cwd": "/w" }),
            options: vec![],
            items: ["git push", "ls", "rm -rf x"]
                .map(|cmd| QuestionItem {
                    state: json!({ "subject": cmd }),
                    options: vec![],
                })
                .to_vec(),
        };
        let scores = |values: &[f64]| {
            let answers: Vec<_> = values
                .iter()
                .map(|v| json!({ "kind": "score", "value": v }))
                .collect();
            json!({ "answer": { "kind": "items", "answers": answers } }).to_string()
        };
        let three = scores(&[0.9, 0.1, 0.8]);
        let fits = advisor(&answering(EXPORT, &three), &Limits::default(), &["risk"]);
        let advice = fits
            .advise(batch(), Duration::from_secs(5))
            .await
            .expect("one answer per item is kept");
        let Answer::Items { answers } = advice.answer else {
            panic!("{:?}", advice.answer)
        };
        assert_eq!(answers[2], Answer::Score { value: 0.8 });
        let two = scores(&[0.9, 0.1]);
        let short = advisor(&answering(EXPORT, &two), &Limits::default(), &["risk"]);
        assert_eq!(short.advise(batch(), Duration::from_secs(5)).await, None);
        let flat = r#"{"answer":{"kind":"score","value":0.9}}"#;
        let single = advisor(&answering(EXPORT, flat), &Limits::default(), &["risk"]);
        assert_eq!(single.advise(batch(), Duration::from_secs(5)).await, None);
    }

    /// One ledger row per call-out, the way `cox-core`'s `call_own` writes
    /// it; `budget` refuses as the session's gate does. Runs the real
    /// `PluginProvider` on the same plugin, so a deadlock would show here.
    #[derive(Default)]
    struct Ledger {
        provider: Mutex<Option<PluginProvider>>,
        over_budget: bool,
        rows: Mutex<Vec<(String, String, String, Job)>>,
        attempts: AtomicU32,
    }

    #[async_trait]
    impl ModelCaller for Ledger {
        async fn call(
            &self,
            _: &str,
            _: Tier,
            _: Request,
        ) -> Result<Vec<ProviderEvent>, CoreError> {
            Err(CoreError::Interrupted)
        }

        async fn call_own(
            &self,
            id: &str,
            section: &str,
            model: &str,
            request: Request,
        ) -> Result<Vec<ProviderEvent>, CoreError> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            if self.over_budget {
                return Err(CoreError::Budget {
                    spent: 1.0,
                    cap: 1.0,
                });
            }
            let provider = self
                .provider
                .lock()
                .expect("ledger lock")
                .take()
                .ok_or(CoreError::Interrupted)?;
            let (tx, mut rx) = mpsc::channel(16);
            let request = Request {
                model: ModelId(model.into()),
                job: Job::Plugin(id.into()),
                ..request
            };
            provider
                .stream(request, tx, CancellationToken::new())
                .await
                .map_err(|error| CoreError::Provider { error })?;
            let mut events = Vec::new();
            while let Some(event) = rx.recv().await {
                events.push(event);
            }
            self.rows.lock().expect("ledger lock").push((
                id.into(),
                section.into(),
                model.into(),
                Job::Plugin(id.into()),
            ));
            Ok(events)
        }
    }

    fn sample_request() -> serde_json::Value {
        serde_json::to_value(Request {
            tier: Tier::Cheap,
            job: Job::Main,
            model: ModelId(String::new()),
            system: vec![],
            tools: vec![],
            messages: vec![],
            effort: Effort::Low,
            max_tokens: 64,
            thinking: Thinking::Off,
            cache_breakpoints: vec![],
            stop_sequences: vec![],
        })
        .expect("request json")
    }

    const RESUMED: &str = r#"{"answer":{"kind":"score","value":0.7},"confidence":0.8}"#;
    const EVENTS: &str = r#"[{"type":"text_delta","text":"0.7"}]"#;

    /// A plugin whose `cox_decide` asks for `section`, whose own section
    /// `own` streams fixed events, and whose resume answers `RESUMED`.
    fn calling_out(section: &str, ledger: Ledger) -> (PluginAdvisor, Arc<Ledger>) {
        let decide = serde_json::to_string(&json!({
            "call": {
                "own_provider": { "name": section, "model": "jev-1" },
                "request": sample_request(),
            },
        }))
        .expect("decide json");
        let body = format!(
            "(memory 1) {} {} {}",
            answering_at(EXPORT, &decide, 0),
            answering_at(crate::provider::EXPORT, EVENTS, 4096),
            answering_at(RESUME, RESUMED, 8192),
        );
        let env = Arc::new(HostEnv::new("p"));
        assert!(env.bind_provider("own", "https://own.example", None, None));
        let host = Arc::new(
            PluginHost::load_with(&module(&body), &Limits::default(), env.clone())
                .expect("module loads"),
        );
        *ledger.provider.lock().expect("ledger lock") = Some(PluginProvider::new(
            "own",
            "https://own.example",
            host.clone(),
            env.clone(),
            Arc::new(|_: &Request| 10),
        ));
        let ledger = Arc::new(ledger);
        let advisor = PluginAdvisor::new("p", host, &["decide:risk".to_owned()])
            .with_call_out(env, ledger.clone());
        (advisor, ledger)
    }

    fn risk_question() -> Question {
        Question {
            point: DecidePoint::Risk,
            state: json!({ "subject": "git push --force" }),
            options: vec![],
            items: vec![],
        }
    }

    #[tokio::test]
    async fn decide_call_out_writes_one_usage_row() {
        let (advisor, ledger) = calling_out("own", Ledger::default());
        let advice = advisor
            .advise(risk_question(), Duration::from_secs(10))
            .await
            .expect("the resumed advice");
        assert_eq!(advice.answer, Answer::Score { value: 0.7 });
        let rows = ledger.rows.lock().expect("ledger lock").clone();
        assert_eq!(
            rows,
            [(
                "p".to_owned(),
                "own".to_owned(),
                "jev-1".to_owned(),
                Job::Plugin("p".into())
            )]
        );
    }

    #[tokio::test]
    async fn decide_call_out_over_budget_falls_back() {
        let ledger = Ledger {
            over_budget: true,
            ..Ledger::default()
        };
        let (advisor, ledger) = calling_out("own", ledger);
        assert_eq!(
            advisor
                .advise(risk_question(), Duration::from_secs(10))
                .await,
            None
        );
        assert!(ledger.rows.lock().expect("ledger lock").is_empty());
    }

    #[tokio::test]
    async fn decide_cannot_target_another_plugins_provider() {
        let (advisor, ledger) = calling_out("someone-elses", Ledger::default());
        assert_eq!(
            advisor
                .advise(risk_question(), Duration::from_secs(10))
                .await,
            None
        );
        assert!(ledger.rows.lock().expect("ledger lock").is_empty());
        assert!(
            ledger.provider.lock().expect("ledger lock").is_some(),
            "the provider never ran"
        );
    }

    #[tokio::test]
    async fn decide_call_out_without_wiring_is_silent() {
        let decide = r#"{"call":{"tier":"cheap","request":{}}}"#;
        let tier = advisor(&answering(EXPORT, decide), &Limits::default(), &["risk"]);
        assert_eq!(
            tier.advise(risk_question(), Duration::from_secs(5)).await,
            None
        );
    }

    /// PL§10: three failures in a row disable `cox_decide` for the
    /// session; the first failure and the disabling each leave one notice.
    #[tokio::test]
    async fn three_failures_disable_decide_export() {
        let ledger = Ledger {
            over_budget: true,
            ..Ledger::default()
        };
        let (advisor, ledger) = calling_out("own", ledger);
        for _ in 0..4 {
            let advice = advisor
                .advise(risk_question(), Duration::from_secs(10))
                .await;
            assert_eq!(advice, None);
        }
        assert_eq!(ledger.attempts.load(Ordering::Relaxed), STRIKES);
        let notices = advisor.call_out.as_ref().expect("wired").env.take_notices();
        let texts: Vec<&str> = notices.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(texts.len(), 2, "{texts:?}");
        assert!(texts[0].starts_with("decision skipped: "), "{texts:?}");
        assert!(texts[1].contains("cox_decide disabled"), "{texts:?}");
    }
}
