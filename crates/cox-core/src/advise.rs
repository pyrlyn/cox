// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The session side of decision points (PL§4 "Decision points", T33.20):
//! asks the advisor `[plugins.decide]` names, applies the point's monotone
//! rule from `router`, and records every answer as `Event::Advised`.
//! Separate from `session.rs` because this is the one place a turn waits on
//! a plugin's answer, and from `router.rs` because routing stays pure.

use std::sync::OnceLock;
use std::time::Duration;

use cox_models::PriceTable;
use cox_protocol::errors::CoreError;
use cox_protocol::plugin::{DecidePoint, Question};
use cox_protocol::types::{Event, Job, Tier};
use cox_sanitize::redact::scrub;
use serde_json::json;

use crate::router::{self, Overrides, Route, Router};
use crate::session::Session;

/// How much of the prompt a `route` question carries (J5.2: ≤ 2k tokens).
const PROMPT_CHARS: usize = 8_000;

impl Session {
    /// This user turn's route: `route` advice applied to the static pick
    /// `route`, which stands on silence, lateness, low confidence or when
    /// the point is off. A followed tier is kept in `Inner::routed` so every
    /// provider call of the turn uses it (`route_for`). Never asked for a
    /// subagent or `Plan` session, a `/think`/`--deep` turn, a `think` static
    /// pick, or after `/model` pinned the tier (J5.2).
    pub(crate) async fn route_turn(
        &self,
        route: Route,
        prompt: &str,
        confirm_think: bool,
    ) -> Result<Route, CoreError> {
        let decide = &self.config.plugins.decide;
        let Some(plugin) = decide.route.as_deref() else {
            return Ok(route);
        };
        let (overrides, prefix) = {
            let inner = self.inner.lock().await;
            (inner.overrides.clone(), inner.last_context_tokens)
        };
        let explicit = confirm_think || route.tier == Tier::Think || overrides.main_tier.is_some();
        if self.job != Job::Main || explicit {
            return Ok(route);
        }
        let Some(advisor) = self.advisor(plugin) else {
            return Ok(route);
        };
        let mut offer = router::route_offer(route.tier);
        if !self.cheap_pays(&route, &overrides, prefix) {
            offer.retain(|tier| *tier != Tier::Cheap);
        }
        if offer.iter().all(|tier| *tier == route.tier) {
            return Ok(route);
        }
        let question = Question {
            point: DecidePoint::Route,
            state: json!({
                "static": tier_name(route.tier),
                // The same redaction the rollout gets (J3: state is built
                // from scrubbed text), clipped to the question's size.
                "prompt": scrub(prompt).chars().take(PROMPT_CHARS).collect::<String>(),
            }),
            options: offer.iter().copied().map(tier_name).collect(),
        };
        let budget = Duration::from_millis(decide.route_ms);
        // The core holds the budget itself, so an advisor that overruns its
        // own deadline is late, never a slow turn.
        let Ok(Some(mut advice)) =
            tokio::time::timeout(budget, advisor.advise(question, budget)).await
        else {
            return Ok(route);
        };
        // `note` belongs to `approve_hint`; untrusted text with no use here
        // stays out of the rollout.
        advice.note = None;
        let (tier, mut applied) =
            router::apply_route(route.tier, &offer, &advice, decide.min_confidence);
        let mut chosen = route;
        if tier != chosen.tier {
            self.inner.lock().await.routed = Some(tier);
            match self.route_for(Job::Main, true).await {
                Ok(advised) => chosen = advised,
                // A tier that cannot route (bad provider name) is no reason
                // to fail a turn the static pick can run.
                Err(_) => {
                    self.inner.lock().await.routed = None;
                    applied = false;
                }
            }
        }
        self.emit(Event::Advised {
            point: DecidePoint::Route,
            plugin: plugin.to_string(),
            advice,
            applied,
        })
        .await?;
        Ok(chosen)
    }

    /// The cache-aware filter (T33.40.8): whether `cheap` is predicted to
    /// beat the static pick by `route_margin`, priced from the catalog the
    /// ledger uses. An unpriced model shows no saving, so `cheap` is not
    /// offered.
    fn cheap_pays(&self, code: &Route, overrides: &Overrides, prefix: u32) -> bool {
        static PRICES: OnceLock<Option<PriceTable>> = OnceLock::new();
        let Some(prices) = PRICES.get_or_init(|| PriceTable::embedded().ok()) else {
            return false;
        };
        let Ok(cheap) = Router::pick(&self.config, Job::Main, Tier::Cheap, overrides, true) else {
            return false;
        };
        match (
            prices.price_for(&cheap.model),
            prices.price_for(&code.model),
        ) {
            (Some(cheap), Some(code)) => {
                router::cheap_pays(cheap, code, prefix, self.config.plugins.decide.route_margin)
            }
            _ => false,
        }
    }
}

/// A tier's wire name (`"cheap"`), the form `Question.options` carries.
fn tier_name(tier: Tier) -> String {
    serde_json::to_value(tier)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use cox_protocol::Store;
    use cox_protocol::errors::ProviderError;
    use cox_protocol::plugin::{Advice, Answer};
    use cox_protocol::traits::{Advisor, Provider};
    use cox_protocol::types::{
        Caps, Content, Message, ProviderEvent, ProviderId, Request, Role, Submission, Usage,
    };
    use cox_provider::scripted::Scripted;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::rollout::History;
    use crate::session::MemoryStore;

    struct Fake {
        delay: Duration,
        advice: Advice,
        asked: Mutex<Vec<Question>>,
    }

    #[async_trait]
    impl Advisor for Fake {
        fn id(&self) -> &str {
            "fake"
        }

        async fn advise(&self, question: Question, _budget: Duration) -> Option<Advice> {
            self.asked
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(question);
            tokio::time::sleep(self.delay).await;
            Some(self.advice.clone())
        }
    }

    impl Fake {
        fn asked(&self) -> Vec<Question> {
            self.asked.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }

    /// The script, plus every request it was sent.
    struct Recording {
        script: Scripted,
        seen: Mutex<Vec<Request>>,
    }

    #[async_trait]
    impl Provider for Recording {
        fn id(&self) -> ProviderId {
            self.script.id()
        }
        fn capabilities(&self) -> Caps {
            self.script.capabilities()
        }
        async fn stream(
            &self,
            req: Request,
            sink: mpsc::Sender<ProviderEvent>,
            cancel: CancellationToken,
        ) -> Result<Usage, ProviderError> {
            self.seen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(req.clone());
            self.script.stream(req, sink, cancel).await
        }
        async fn count_tokens(&self, req: &Request) -> Result<u32, ProviderError> {
            self.script.count_tokens(req).await
        }
    }

    fn cheap(confidence: f64) -> Advice {
        Advice {
            answer: Answer::Choice { order: vec![0] },
            confidence: Some(confidence),
            note: Some("\u{1b}[31mignored".into()),
        }
    }

    /// A session whose `route` point the fake answers, over `turns` scripted
    /// replies, recording every request.
    fn session(
        turns: usize,
        delay: Duration,
        advice: Advice,
    ) -> (Session, Arc<MemoryStore>, Arc<Fake>, Arc<Recording>) {
        let mut config = cox_protocol::Config::default();
        config.plugins.decide.route = Some("fake".into());
        config.plugins.decide.route_ms = 50;
        let script = "[[turn]]\ntext = \"done\"\n".repeat(turns);
        let provider = Arc::new(Recording {
            script: Scripted::from_toml(&script, "").expect("scenario"),
            seen: Mutex::new(Vec::new()),
        });
        let store = Arc::new(MemoryStore::new());
        let session = Session::new(
            config,
            provider.clone(),
            vec![],
            store.clone(),
            store.clone(),
            PathBuf::from("/tmp/cox-advise"),
        )
        .expect("session");
        let fake = Arc::new(Fake {
            delay,
            advice,
            asked: Mutex::new(Vec::new()),
        });
        session.set_advisors(vec![fake.clone()]);
        (session, store, fake, provider)
    }

    async fn submit(session: &Session, confirm_think: bool) {
        session
            .submit(Submission::UserTurn {
                text: "rename foo to bar".into(),
                attachments: vec![],
                confirm_think,
            })
            .await
            .expect("turn");
    }

    /// One user turn under an advisor; returns the rollout, the ledger's
    /// tiers and the questions the advisor saw.
    async fn turn(delay: Duration, advice: Advice) -> (Vec<Event>, Vec<Tier>, Vec<Question>) {
        let (session, store, fake, _) = self::session(1, delay, advice);
        submit(&session, false).await;
        assert_eq!(
            session.inner.lock().await.routed,
            None,
            "advice outlives its turn"
        );
        let events = store.rollout_read(&session.id()).expect("rollout");
        let tiers = store.usage_rows().iter().map(|r| r.tier).collect();
        (events, tiers, fake.asked())
    }

    fn started_tier(events: &[Event]) -> Option<Tier> {
        events.iter().find_map(|e| match e {
            Event::TurnStarted { tier, .. } => Some(*tier),
            _ => None,
        })
    }

    fn advised(events: &[Event]) -> Vec<&Event> {
        events
            .iter()
            .filter(|e| matches!(e, Event::Advised { .. }))
            .collect()
    }

    #[tokio::test]
    async fn late_advice_falls_back_to_static_pick() {
        let (events, tiers, asked) = turn(Duration::from_millis(500), cheap(0.99)).await;
        assert_eq!(asked.len(), 1, "the point was asked");
        assert_eq!(started_tier(&events), Some(Tier::Code));
        assert_eq!(tiers, vec![Tier::Code]);
        assert!(advised(&events).is_empty(), "a late answer is no answer");
    }

    #[tokio::test]
    async fn low_confidence_advice_is_recorded_but_not_applied() {
        let (events, tiers, _) = turn(Duration::ZERO, cheap(0.3)).await;
        assert_eq!(started_tier(&events), Some(Tier::Code));
        assert_eq!(tiers, vec![Tier::Code]);
        assert!(matches!(
            advised(&events)[..],
            [Event::Advised { applied: false, .. }]
        ));
    }

    #[tokio::test]
    async fn advised_event_in_rollout_replays_identically() {
        let (events, tiers, asked) = turn(Duration::ZERO, cheap(0.9)).await;
        assert_eq!(asked[0].options, vec!["cheap", "code"]);
        assert_eq!(asked[0].state["static"], "code");
        // The whole turn, every provider call included, ran on the advice.
        assert_eq!(started_tier(&events), Some(Tier::Cheap));
        assert_eq!(tiers, vec![Tier::Cheap]);
        let [
            Event::Advised {
                point,
                plugin,
                advice,
                applied,
            },
        ] = advised(&events)[..]
        else {
            panic!("one Advised event: {events:?}");
        };
        assert_eq!(
            (point, plugin.as_str(), applied),
            (&DecidePoint::Route, "fake", &true)
        );
        assert_eq!(advice.note, None);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::ModelSwitched { .. }))
        );
        // The rollout line round-trips byte-for-byte, and replaying with or
        // without it rebuilds the same history (D12: replay without the
        // plugin gives the same log).
        for ev in &events {
            let line = serde_json::to_string(ev).expect("serialize");
            let back: Event = serde_json::from_str(&line).expect("deserialize");
            assert_eq!(serde_json::to_string(&back).expect("serialize"), line);
        }
        let without: Vec<Event> = events
            .iter()
            .filter(|e| !matches!(e, Event::Advised { .. }))
            .cloned()
            .collect();
        assert_eq!(
            History::from_rollout(&events, false),
            History::from_rollout(&without, false)
        );
    }

    #[tokio::test]
    async fn downgrade_offered_on_first_turn() {
        let (_, tiers, asked) = turn(Duration::ZERO, cheap(0.9)).await;
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].options, vec!["cheap", "code"]);
        assert_eq!(tiers, vec![Tier::Cheap]);
    }

    #[tokio::test]
    async fn downgrade_not_offered_when_cache_loss_exceeds_saving() {
        let (session, store, fake, _) = session(1, Duration::ZERO, cheap(0.99));
        // J5.2's example: a 30k warm prefix makes cheap cost more than code.
        session.inner.lock().await.last_context_tokens = 30_000;
        submit(&session, false).await;
        assert!(fake.asked().is_empty(), "code alone is no choice");
        let tiers: Vec<Tier> = store.usage_rows().iter().map(|r| r.tier).collect();
        assert_eq!(tiers, vec![Tier::Code]);
    }

    #[tokio::test]
    async fn routed_down_turn_keeps_history_thinking() {
        let (session, store, fake, rec) = session(3, Duration::ZERO, cheap(0.9));
        let signed = Content::Thinking {
            text: "plan".into(),
            signature: Some("code-model-signature".into()),
        };
        {
            let mut inner = session.inner.lock().await;
            inner.history.push(Message {
                role: Role::User,
                content: vec![Content::Text {
                    text: "earlier".into(),
                }],
            });
            inner.history.push(Message {
                role: Role::Assistant,
                content: vec![signed.clone(), Content::Text { text: "ok".into() }],
            });
        }
        // A warm prefix keeps turns 1 and 3 on code; turn 2 is routed down.
        for prefix in [30_000, 0, 30_000] {
            session.inner.lock().await.last_context_tokens = prefix;
            submit(&session, false).await;
        }
        assert_eq!(fake.asked().len(), 1);
        let seen = rec.seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let tiers: Vec<Tier> = seen.iter().map(|r| r.tier).collect();
        assert_eq!(tiers, vec![Tier::Code, Tier::Cheap, Tier::Code]);
        let thinks = |r: &Request| r.messages.iter().any(|m| m.content.contains(&signed));
        assert!(thinks(&seen[0]) && !thinks(&seen[1]) && thinks(&seen[2]));
        // The next code request reuses the first one's prefix byte for byte.
        let prefix = |r: &Request| serde_json::to_vec(&(&r.system[0..=2], &r.tools)).ok();
        assert_eq!(prefix(&seen[2]), prefix(&seen[0]));
        assert_eq!(
            seen[2].messages[..seen[0].messages.len()],
            seen[0].messages[..]
        );
        assert!(
            session.inner.lock().await.history[1]
                .content
                .contains(&signed)
        );
        let events = store.rollout_read(&session.id()).expect("rollout");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::ModelSwitched { .. }))
        );
    }

    #[tokio::test]
    async fn route_never_offered_for_plan_job() {
        let (session, _, fake, _) = session(2, Duration::ZERO, cheap(0.9));
        let plan = session
            .spawn_child(
                session.config.clone(),
                vec![],
                Job::Plan,
                Tier::Code,
                None,
                None,
                "plan".into(),
                "plan".into(),
            )
            .expect("child");
        plan.set_advisors(vec![fake.clone()]);
        submit(&plan, false).await;
        // `/think` and `--deep` confirm think: the user asked for depth.
        submit(&session, true).await;
        assert!(fake.asked().is_empty());
    }

    #[tokio::test]
    async fn model_override_disables_route_point() {
        let (session, store, fake, _) = session(1, Duration::ZERO, cheap(0.9));
        session
            .submit(Submission::SwitchModel {
                tier: Tier::Code,
                model: None,
            })
            .await
            .expect("switch");
        submit(&session, false).await;
        assert!(fake.asked().is_empty());
        let tiers: Vec<Tier> = store.usage_rows().iter().map(|r| r.tier).collect();
        assert_eq!(tiers, vec![Tier::Code]);
    }
}
