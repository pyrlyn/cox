// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Job → tier → model routing (D5, T9.1). One pure function owns every
//! routing choice: which tier a job runs on, which model string it sends,
//! and whether the `think` tier's confirmation gate blocks the turn. The
//! loop never guesses a model itself; a failing cheap call is retried on
//! cheap because `pick` is stateless. `route_offer`/`apply_route` are the
//! `route` decision point's monotone rule (PL§4, T33.20): advice may only
//! move a main turn down, and `cheap_pays` is its cache-aware filter
//! (T33.40.8).

use std::collections::HashMap;

use cox_models::Price;
use cox_protocol::Config;
use cox_protocol::plugin::{Advice, Answer};
use cox_protocol::types::{Content, Effort, Job, Message, ModelId, ProviderId, Thinking, Tier};

/// Fable 5.1 list prices shown by the think gate (Anthropic first-party,
/// re-verify in T1.7's `prices.toml` when it changes).
pub const THINK_PRICE: &str = "$10/$50 per MTok";

/// Where one provider call goes.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    /// Resolved tier.
    pub tier: Tier,
    /// Backend the tier names.
    pub provider: ProviderId,
    /// Model id sent on the wire.
    pub model: ModelId,
    /// Effort from the tier block.
    pub effort: Effort,
    /// Thinking mode from the tier block.
    pub thinking: Thinking,
    /// Max output tokens from the tier block.
    pub max_tokens: u32,
}

/// Session-scoped routing overrides (`/model`, `--tier <tier>=<model>`).
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    /// Per-tier model replacements; absent means the tier's configured model.
    pub models: HashMap<Tier, ModelId>,
    /// `/model <tier>`: main turns run on this tier instead of `jobs.main`.
    pub main_tier: Option<Tier>,
    /// `/effort`: main turns ask for this effort instead of the tier's;
    /// `clamp_effort` still bounds it per model (T16.4).
    pub effort: Option<Effort>,
}

/// Why `pick` refused to route.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteError {
    /// The `think` tier needs `confirm_think` (invariant #9).
    NeedsConfirm {
        /// Tier that required confirmation.
        tier: Tier,
        /// Model that would have been called.
        model: ModelId,
    },
    /// A tier names no known `[providers.*]` table.
    UnknownProvider {
        /// Tier carrying the bad name.
        tier: Tier,
        /// The unknown name.
        name: String,
    },
}

impl RouteError {
    /// What the model and the user see for this refusal.
    pub fn notice(&self) -> String {
        match self {
            Self::NeedsConfirm { model, .. } => format!(
                "think tier ({}) requires confirmation: resubmit with confirm_think \
                 ({THINK_PRICE})",
                model.0
            ),
            Self::UnknownProvider { tier, name } => {
                format!("tier {tier:?} names unknown provider `{name}`")
            }
        }
    }
}

/// The router: stateless job → route resolution (plan.md T9.1).
pub struct Router;

impl Router {
    /// Resolves `job` to a [`Route`]. Main turns run on the session tier
    /// (or `overrides.main_tier`: the `/model` tier, or the one-turn tier the
    /// session sets for `route` advice and `/think`); every other job follows
    /// the `[jobs]` table.
    /// Pure: the same inputs always give the same route, so a retry never
    /// escalates a tier on its own.
    pub fn pick(
        config: &Config,
        job: Job,
        session_tier: Tier,
        overrides: &Overrides,
        confirm_think: bool,
    ) -> Result<Route, RouteError> {
        // Matches by reference (`job` is no longer `Copy`, T33.15) so the
        // `want` match below can still use it; only the `[jobs]` lookup
        // needs an owned clone.
        let tier = match &job {
            Job::Main => overrides.main_tier.unwrap_or(session_tier),
            // A plugin's `cox_model_call` already carries its own
            // grant-clamped tier (never `think`, PL§7d); `[jobs]` has no
            // per-plugin entry, so the caller passes the resolved tier
            // through `session_tier`, the same slot `Job::Main` uses for
            // `/model` (T33.15).
            Job::Plugin(_) => session_tier,
            _ => config.jobs.tier_for(job.clone()),
        };
        let tc = config.tiers.get(tier);
        if tier == Tier::Think && tc.confirm && !confirm_think {
            return Err(RouteError::NeedsConfirm {
                tier,
                model: ModelId(tc.model.clone()),
            });
        }
        let provider = match tc.provider.as_str() {
            "anthropic" => ProviderId::Anthropic,
            "openai" => ProviderId::OpenAi,
            "local" => ProviderId::Local,
            // T30.15: LM Studio's `/v1/messages` goes through the
            // Anthropic wire client (`session::backend_for`), whose
            // `Provider::id()` always reports `Anthropic` — matching that
            // here keeps this route label and the live provider's id (the
            // ledger row's `provider` field) in agreement.
            "lmstudio" => ProviderId::Anthropic,
            // Jev is type-1 native (System One wire, T21.1): its own id so
            // the ledger row reads as a decision call, not an LLM turn.
            "typesafe" => ProviderId::Jev,
            // Type-2 (compatible) providers are OpenAI-Chat-shaped by
            // construction, so they ride the `Local` family id — the
            // ledger's "OpenAI-compatible" bucket, where the model string
            // disambiguates the row (precedent: Scripted/Replay already do).
            other if config.providers.custom.contains_key(other) => ProviderId::Local,
            other => {
                return Err(RouteError::UnknownProvider {
                    tier,
                    name: other.to_string(),
                });
            }
        };
        // The model the wire carries: a session override (`/model`,
        // `--tier TIER=MODEL`) wins; otherwise a Local-family tier pins its
        // section's default model — a Claude id would 404 against Ollama,
        // and a bare `tiers.code.provider = "deepseek"` flip must work
        // without also editing every tier model. Native tiers carry their
        // own configured model. An empty section default falls back to the
        // tier model rather than sending an empty id.
        let pinned = match tc.provider.as_str() {
            "local" => Some(config.providers.local.model.clone()),
            // Same pin rule as `local`, but `lmstudio.model` is normally
            // left empty (T30.15: pin through `tiers.code.model` /
            // `--tier code=<model>` instead) — an empty pin falls through
            // to `tc.model` below, same as an unset `local`/`typesafe` one.
            "lmstudio" => Some(config.providers.lmstudio.model.clone()),
            // Same pin rule as `local`: a bare `tiers.<t>.provider =
            // "typesafe"` flip works without editing every tier model.
            "typesafe" => Some(config.providers.typesafe.model.clone()),
            other => config.providers.custom.get(other).map(|c| c.model.clone()),
        };
        let model = overrides.models.get(&tier).cloned().unwrap_or_else(|| {
            pinned
                .filter(|m| !m.is_empty())
                .map(ModelId)
                .unwrap_or_else(|| ModelId(tc.model.clone()))
        });
        let want = match job {
            Job::Main => overrides.effort.unwrap_or(tc.effort),
            _ => tc.effort,
        };
        Ok(Route {
            tier,
            provider,
            model: model.clone(),
            effort: clamp_effort(config, &tc.provider, &model, want),
            thinking: tc.thinking,
            max_tokens: tc.max_tokens,
        })
    }
}

/// Clamps the tier's effort to what the routed model supports: the greatest
/// supported level at or below the request, else the lowest supported one.
/// Models absent from the section list — or an empty list — pass through
/// untouched, so a gateway can serve models cox never catalogued.
fn clamp_effort(config: &Config, provider: &str, model: &ModelId, want: Effort) -> Effort {
    let supported: Vec<Effort> = config
        .providers
        .models_for(provider)
        .iter()
        .find(|m| m.id == model.0)
        .map(|m| m.efforts.clone())
        .unwrap_or_default();
    if supported.is_empty() || supported.contains(&want) {
        return want;
    }
    supported
        .iter()
        .filter(|e| **e <= want)
        .max()
        .or_else(|| supported.iter().min())
        .copied()
        .unwrap_or(want)
}

/// The tiers the `route` decision point may offer (PL§4, D5): at or below
/// the static pick and never `think`, cheapest first. When every offered
/// tier is the static pick there is nothing to choose and the point is not
/// asked. `cheap_pays` narrows this further (T33.40.8).
pub fn route_offer(static_tier: Tier) -> Vec<Tier> {
    [Tier::Cheap, Tier::Code]
        .into_iter()
        .filter(|tier| *tier <= static_tier)
        .collect()
}

/// Applies `route` advice to the static pick: the first choice, when it is
/// one of `offer` and its confidence reaches `min_confidence`. Returns the
/// tier and whether the advice was followed; anything else keeps the static
/// pick. Re-checks "never up" itself so no caller's `offer` can widen it.
pub fn apply_route(
    static_tier: Tier,
    offer: &[Tier],
    advice: &Advice,
    min_confidence: f64,
) -> (Tier, bool) {
    let confident = advice.confidence.is_some_and(|c| c >= min_confidence);
    let choice = match &advice.answer {
        Answer::Choice { order } => order
            .first()
            .and_then(|i| offer.get(usize::try_from(*i).ok()?))
            .copied(),
        Answer::Score { .. } | Answer::Noul { .. } | Answer::Scores { .. } => None,
    };
    match choice {
        Some(tier) if confident && tier <= static_tier && tier != Tier::Think => (tier, true),
        _ => (static_tier, false),
    }
}

/// Provider calls one user turn is predicted to make (J5.2's worked example).
const TURN_CALLS: f64 = 5.0;
/// Output tokens, thinking included, one user turn is predicted to write.
const TURN_OUTPUT: f64 = 3_000.0;

/// Whether a turn on `cheap` is predicted to cost at most `1 − margin` of
/// the same turn on `code` (J5.2). The `code` turn reads its warm prefix
/// on every call; the switch writes `prefix` into the cheap tier's cache
/// once, then reads it. A large prefix therefore makes a downgrade lose
/// money even at half the list price, and the first turn (no prefix yet)
/// always passes.
pub fn cheap_pays(cheap: &Price, code: &Price, prefix: u32, margin: f64) -> bool {
    let p = f64::from(prefix);
    let code_cost = code.cache_read * p * TURN_CALLS + code.output * TURN_OUTPUT;
    let cheap_cost = cheap.cache_write * p
        + cheap.cache_read * p * (TURN_CALLS - 1.0)
        + cheap.output * TURN_OUTPUT;
    // NaN clamps to NaN and compares false: a broken margin never offers.
    cheap_cost <= (1.0 - margin.clamp(0.0, 1.0)) * code_cost
}

/// Drops `Thinking` blocks after a model switch: a signature binds its block
/// to the model and prefix that produced it, so replaying it under another
/// model is a guaranteed provider error. Messages left empty are dropped;
/// everything else is byte-identical.
pub fn strip_thinking(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .filter_map(|msg| {
            let kept: Vec<Content> = msg
                .content
                .iter()
                .filter(|c| !matches!(c, Content::Thinking { .. }))
                .cloned()
                .collect();
            (!kept.is_empty()).then_some(Message {
                role: msg.role,
                content: kept,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cox_protocol::config::CompatibleProviderConfig;
    use cox_protocol::types::Role;

    fn custom_config() -> Config {
        let mut cfg = Config::default();
        cfg.tiers.code.provider = "deepseek".into();
        cfg.providers.custom.insert(
            "deepseek".into(),
            CompatibleProviderConfig {
                base_url: "https://api.deepseek.com".into(),
                api_key_env: "DEEPSEEK_API_KEY".into(),
                api: "chat".into(),
                model: "deepseek-v4-pro".into(),
                context_window: 1_000_000,
                models: vec![
                    cox_protocol::config::ProviderModel {
                        id: "deepseek-v4-flash".into(),
                        context_window: 1_000_000,
                        efforts: vec![Effort::Low, Effort::High, Effort::Xhigh],
                        ..Default::default()
                    },
                    cox_protocol::config::ProviderModel {
                        id: "deepseek-v4-pro".into(),
                        context_window: 1_000_000,
                        efforts: vec![Effort::High, Effort::Xhigh],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
        );
        cfg
    }

    #[test]
    fn router_custom_provider_pins_section_model_and_clamps_effort() {
        let cfg = custom_config();
        // A bare provider flip routes without touching tier models: the
        // section default pins the wire id (a Claude id would 404).
        let route = Router::pick(&cfg, Job::Main, Tier::Code, &Overrides::default(), true)
            .expect("custom routes");
        assert_eq!(route.provider, ProviderId::Local);
        assert_eq!(route.model.0, "deepseek-v4-pro");
        // Code tier asks High, pro supports it: unchanged.
        assert_eq!(route.effort, Effort::High);
        // An override still wins over the pin (gateway escape hatch).
        let mut overrides = Overrides::default();
        overrides
            .models
            .insert(Tier::Code, ModelId("deepseek-v4-flash".into()));
        let route = Router::pick(&cfg, Job::Main, Tier::Code, &overrides, true).expect("routes");
        assert_eq!(route.model.0, "deepseek-v4-flash");
        assert_eq!(route.effort, Effort::High);
    }

    #[test]
    fn router_session_effort_applies_to_main_turns_and_is_clamped() {
        let cfg = custom_config();
        let low = Overrides {
            effort: Some(Effort::Low),
            ..Overrides::default()
        };
        // pro has no `low`: the floor it does support.
        let route = Router::pick(&cfg, Job::Main, Tier::Code, &low, true).expect("routes");
        assert_eq!(route.effort, Effort::High);
        let xhigh = Overrides {
            effort: Some(Effort::Xhigh),
            ..Overrides::default()
        };
        let route = Router::pick(&cfg, Job::Main, Tier::Code, &xhigh, true).expect("routes");
        assert_eq!(route.effort, Effort::Xhigh);
        // Background jobs keep their tier's effort.
        let route = Router::pick(&cfg, Job::Compact, Tier::Code, &xhigh, true).expect("routes");
        let tier = cfg.jobs.tier_for(Job::Compact);
        assert_eq!(route.effort, cfg.tiers.get(tier).effort);
    }

    /// T30.15: a bare `tiers.code.provider = "lmstudio"` flip (no
    /// `providers.lmstudio.model` set) routes on `tiers.code.model` alone
    /// — the card's "Done when" — and reads as an Anthropic-wire call
    /// (`session::backend_for`'s `lmstudio` arm builds an
    /// `AnthropicProvider`), matching `Provider::id()`.
    #[test]
    fn router_lmstudio_maps_to_anthropic_and_pins_on_tier_model_alone() {
        let mut cfg = Config::default();
        cfg.tiers.code.provider = "lmstudio".into();
        cfg.tiers.code.model = "prism-ml/bonsai-27b".into();
        let route = Router::pick(&cfg, Job::Main, Tier::Code, &Overrides::default(), true)
            .expect("lmstudio routes");
        assert_eq!(route.provider, ProviderId::Anthropic);
        assert_eq!(route.model.0, "prism-ml/bonsai-27b");

        // A configured section model pins the same way `local`'s does.
        cfg.providers.lmstudio.model = "qwen3-coder-local".into();
        let route = Router::pick(&cfg, Job::Main, Tier::Code, &Overrides::default(), true)
            .expect("lmstudio routes");
        assert_eq!(route.model.0, "qwen3-coder-local");
    }

    #[test]
    fn router_clamp_effort_never_upgrades_past_the_request() {
        let cfg = custom_config();
        // Xhigh on flash (supports all three): passes through.
        assert_eq!(
            clamp_effort(
                &cfg,
                "deepseek",
                &ModelId("deepseek-v4-flash".into()),
                Effort::Xhigh
            ),
            Effort::Xhigh
        );
        // Low on pro (supports high/xhigh): raised to the floor, the only
        // direction that keeps the call valid.
        assert_eq!(
            clamp_effort(
                &cfg,
                "deepseek",
                &ModelId("deepseek-v4-pro".into()),
                Effort::Low
            ),
            Effort::High
        );
        // Unlisted models and unknown providers pass through untouched.
        assert_eq!(
            clamp_effort(
                &cfg,
                "deepseek",
                &ModelId("deepseek-future-1".into()),
                Effort::Xhigh
            ),
            Effort::Xhigh
        );
        assert_eq!(
            clamp_effort(&cfg, "local", &ModelId("qwen3-coder".into()), Effort::High),
            Effort::High
        );
    }

    #[test]
    fn router_clamp_effort_treats_medium_as_a_level_between_low_and_high() {
        let mut cfg = custom_config();
        if let Some(section) = cfg.providers.custom.get_mut("deepseek") {
            section.models.push(cox_protocol::config::ProviderModel {
                id: "deepseek-v4-mid".into(),
                context_window: 1_000_000,
                efforts: vec![Effort::Low, Effort::Medium],
                ..Default::default()
            });
        }
        let clamp =
            |model: &str, want| clamp_effort(&cfg, "deepseek", &ModelId(model.into()), want);
        // A row that lists `medium` keeps it.
        assert_eq!(clamp("deepseek-v4-mid", Effort::Medium), Effort::Medium);
        // Above the row's top level: down to `medium`, not to `low`.
        assert_eq!(clamp("deepseek-v4-mid", Effort::Xhigh), Effort::Medium);
        // A row without `medium` (low/high/xhigh): down to `low`, never up.
        assert_eq!(clamp("deepseek-v4-flash", Effort::Medium), Effort::Low);
        // A row whose floor is `high`: raised to it, as for `low`.
        assert_eq!(clamp("deepseek-v4-pro", Effort::Medium), Effort::High);
    }

    #[test]
    fn router_strip_thinking_keeps_everything_else_verbatim() {
        let thinking = Content::Thinking {
            text: "hmm".into(),
            signature: Some("sig".into()),
        };
        let text = Content::Text { text: "hi".into() };
        let messages = vec![
            Message {
                role: Role::User,
                content: vec![text.clone()],
            },
            Message {
                role: Role::Assistant,
                content: vec![thinking, text.clone()],
            },
            Message {
                role: Role::Assistant,
                content: vec![Content::Thinking {
                    text: "only".into(),
                    signature: None,
                }],
            },
        ];
        let out = strip_thinking(&messages);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].content, vec![text.clone()]);
        assert_eq!(out[1].content, vec![text]);
    }

    fn price(input: f64, output: f64, cache_write: f64, cache_read: f64) -> Price {
        Price {
            id: String::new(),
            input,
            output,
            cache_write,
            cache_read,
            verified_on: String::new(),
            source_url: String::new(),
        }
    }

    #[test]
    fn cheap_pays_follows_the_j5_2_cache_formula() {
        // J19's prices: Haiku 4.5 is only 2x cheaper than Sonnet 5.
        let (haiku, sonnet) = (price(1.0, 5.0, 1.25, 0.10), price(2.0, 10.0, 2.50, 0.20));
        // J5.2's example, P = 30k: cheap $0.0645 against code $0.060.
        assert!(!cheap_pays(&haiku, &sonnet, 30_000, 0.0));
        // No prefix yet: half the output price wins.
        assert!(cheap_pays(&haiku, &sonnet, 0, 0.15));
        // Break-even at the default margin is P = 13 125.
        assert!(cheap_pays(&haiku, &sonnet, 13_000, 0.15));
        assert!(!cheap_pays(&haiku, &sonnet, 13_250, 0.15));
        assert!(!cheap_pays(&haiku, &sonnet, 0, f64::NAN));
    }

    fn advice(order: Vec<u32>, confidence: Option<f64>) -> Advice {
        Advice {
            answer: Answer::Choice { order },
            confidence,
            note: None,
        }
    }

    #[test]
    fn route_advice_never_routes_up() {
        assert_eq!(route_offer(Tier::Cheap), vec![Tier::Cheap]);
        assert_eq!(route_offer(Tier::Code), vec![Tier::Cheap, Tier::Code]);
        assert_eq!(route_offer(Tier::Think), vec![Tier::Cheap, Tier::Code]);
        // Even an offer a caller widened past the rule cannot route up.
        let wide = [Tier::Cheap, Tier::Code, Tier::Think];
        for static_tier in wide {
            for offer in [route_offer(static_tier), wide.to_vec()] {
                for i in 0..4 {
                    let (tier, _) =
                        apply_route(static_tier, &offer, &advice(vec![i], Some(1.0)), 0.6);
                    assert!(tier <= static_tier, "{static_tier:?} -> {tier:?}");
                    assert!(tier != Tier::Think || static_tier == Tier::Think);
                }
            }
        }
        let offer = route_offer(Tier::Code);
        assert_eq!(
            apply_route(Tier::Code, &offer, &advice(vec![0, 1], Some(0.9)), 0.6),
            (Tier::Cheap, true)
        );
        for low in [
            advice(vec![0], Some(0.5)),
            advice(vec![0], None),
            advice(vec![], Some(1.0)),
        ] {
            assert_eq!(
                apply_route(Tier::Code, &offer, &low, 0.6),
                (Tier::Code, false)
            );
        }
        let score = Advice {
            answer: Answer::Score { value: 0.0 },
            confidence: Some(1.0),
            note: None,
        };
        assert_eq!(
            apply_route(Tier::Code, &offer, &score, 0.6),
            (Tier::Code, false)
        );
    }
}
