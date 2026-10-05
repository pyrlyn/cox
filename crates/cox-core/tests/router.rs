// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Router tests (T9.1): the job table, the think gate, no auto-escalation,
//! overrides and local mode, plus `/model` through a live session.

mod common;

use common::{drain, open, spawn_turn};
use cox_core::Session;
use cox_core::router::{Overrides, RouteError, Router};
use cox_protocol::types::{
    Effort, Event, Job, ModelId, PermissionMode, StopReason, Submission, Tier,
};
use cox_protocol::{Config, ProviderId};

fn table() -> Vec<(Job, Tier)> {
    vec![
        (Job::Main, Tier::Code),
        (Job::Plan, Tier::Think),
        (Job::Compact, Tier::Cheap),
        (Job::Title, Tier::Cheap),
        (Job::Summarize, Tier::Cheap),
        (Job::Commit, Tier::Cheap),
        (Job::Memory, Tier::Cheap),
        (Job::Explore, Tier::Cheap),
        (Job::Shell, Tier::Cheap),
        (Job::Hook, Tier::Cheap),
    ]
}

#[test]
fn router_job_table_pins_every_job() {
    // Ten `Job` variants exist; the plan's "12" counts the two switch-model
    // forms below as rows of the same table.
    let config = Config::default();
    assert_eq!(table().len(), 10);
    for (job, tier) in table() {
        let route = Router::pick(
            &config,
            job.clone(),
            Tier::Code,
            &Overrides::default(),
            true,
        )
        .expect("routed");
        assert_eq!(route.tier, tier, "{job:?}");
    }
    let main = Router::pick(&config, Job::Main, Tier::Code, &Overrides::default(), true).unwrap();
    assert_eq!(main.model.0, "claude-sonnet-5");
    let compact = Router::pick(
        &config,
        Job::Compact,
        Tier::Code,
        &Overrides::default(),
        true,
    )
    .unwrap();
    assert_eq!(compact.model.0, "claude-haiku-4-5");
    assert_eq!(compact.provider, ProviderId::Anthropic);
}

#[test]
fn router_think_requires_confirmation() {
    let config = Config::default();
    let err = Router::pick(&config, Job::Plan, Tier::Code, &Overrides::default(), false)
        .expect_err("gate");
    assert!(matches!(err, RouteError::NeedsConfirm { .. }));
    assert!(err.notice().contains("$10/$50"), "price shown");
    Router::pick(&config, Job::Plan, Tier::Code, &Overrides::default(), true).expect("confirmed");
    let mut open = Config::default();
    open.tiers.think.confirm = false;
    Router::pick(&open, Job::Plan, Tier::Code, &Overrides::default(), false).expect("no gate");
}

#[test]
fn router_never_auto_escalates() {
    let config = Config::default();
    let overrides = Overrides::default();
    // A retry is the same pure call: nothing about a failure can move tiers.
    let first = Router::pick(&config, Job::Explore, Tier::Code, &overrides, true).unwrap();
    let retry = Router::pick(&config, Job::Explore, Tier::Code, &overrides, true).unwrap();
    assert_eq!(first, retry);
    assert_eq!(first.tier, Tier::Cheap);
}

#[test]
fn router_model_override_local_and_unknown() {
    let config = Config::default();
    let mut overrides = Overrides::default();
    overrides
        .models
        .insert(Tier::Code, ModelId("claude-opus-5".into()));
    overrides.main_tier = Some(Tier::Think);
    let route = Router::pick(&config, Job::Main, Tier::Code, &overrides, true).unwrap();
    assert_eq!(route.tier, Tier::Think);
    assert_eq!(
        route.model.0, "claude-fable-5-1",
        "tier default without its own entry"
    );

    let mut local = Config::default();
    for tiers in [
        &mut local.tiers.cheap,
        &mut local.tiers.code,
        &mut local.tiers.think,
    ] {
        tiers.provider = "local".into();
    }
    local.providers.local.model = "qwen3-coder".into();
    for (job, _) in table() {
        let route =
            Router::pick(&local, job.clone(), Tier::Code, &Overrides::default(), true).unwrap();
        assert_eq!(route.provider, ProviderId::Local, "{job:?}");
        assert_eq!(route.model.0, "qwen3-coder", "{job:?}");
    }

    let mut bad = Config::default();
    bad.tiers.code.provider = "weird".into();
    assert!(matches!(
        Router::pick(&bad, Job::Main, Tier::Code, &Overrides::default(), true),
        Err(RouteError::UnknownProvider { .. })
    ));
}

#[tokio::test]
async fn router_switch_gates_and_runs_think() {
    let toml = "[[turn]]\ntext = \"deep thought\"\n";
    let (session, store, mut rx) = open(toml, Config::default());
    session
        .submit(Submission::SwitchModel {
            tier: Tier::Think,
            model: None,
        })
        .await
        .expect("switch");
    // Unconfirmed think is refused before any provider call, price shown.
    let running = spawn_turn(&session, "plan it");
    let events = drain(&mut rx).await;
    running.await.expect("join").expect("turn");
    assert!(events.iter().any(|e| matches!(
        e,
        Event::ModelSwitched {
            tier: Tier::Think,
            ..
        }
    )));
    assert!(matches!(
        events.last(),
        Some(Event::TurnDone {
            stop: StopReason::Refusal { .. },
            ..
        })
    ));
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::Notice { text, .. } if text.contains("$10/$50")
        )),
        "price notice"
    );
    assert!(store.usage_rows().is_empty(), "no provider call");

    // Confirmed think runs on the think tier and model.
    let running = spawn_turn_confirmed(&session, "plan it");
    let events = drain(&mut rx).await;
    running.await.expect("join").expect("turn");
    assert!(matches!(
        events.last(),
        Some(Event::TurnDone {
            stop: StopReason::EndTurn,
            ..
        })
    ));
    let rows = store.usage_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tier, Tier::Think);
    assert_eq!(rows[0].model.0, "claude-fable-5-1");
}

#[tokio::test]
async fn confirm_think_runs_one_turn_on_think_then_the_session_tier_again() {
    // Turn 1 is a tool call and its follow-up, so every request of the
    // confirmed turn is checked, not just the first.
    let toml = "[[turn]]\ntext = \"look\"\ntool_calls = [{ name = \"echo\", input = { text = \"x\" } }]\n\
                [[turn]]\ntext = \"planned\"\n[[turn]]\ntext = \"coded\"\n";
    let (session, store, mut rx) = open(toml, Config::default());
    let running = spawn_turn_confirmed(&session, "plan it");
    let events = drain(&mut rx).await;
    running.await.expect("join").expect("turn");
    assert!(events.iter().any(|e| matches!(
        e,
        Event::TurnStarted {
            tier: Tier::Think,
            ..
        }
    )));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::ModelSwitched { .. })),
        "the session tier is not switched"
    );

    let running = spawn_turn(&session, "now code it");
    drain(&mut rx).await;
    running.await.expect("join").expect("turn");
    let rows: Vec<(Job, Tier, String)> = store
        .usage_rows()
        .into_iter()
        .map(|r| (r.job, r.tier, r.model.0))
        .collect();
    assert_eq!(
        rows,
        [
            (Job::Main, Tier::Think, "claude-fable-5-1".to_string()),
            (Job::Main, Tier::Think, "claude-fable-5-1".to_string()),
            (Job::Main, Tier::Code, "claude-sonnet-5".to_string()),
        ]
    );
}

fn spawn_turn_confirmed(
    session: &Session,
    text: &str,
) -> tokio::task::JoinHandle<Result<(), cox_protocol::errors::CoreError>> {
    let session = session.clone();
    let text = text.to_owned();
    tokio::spawn(async move {
        session
            .submit(Submission::UserTurn {
                text,
                attachments: vec![],
                confirm_think: true,
            })
            .await
    })
}

#[tokio::test]
async fn router_set_effort_changes_the_next_request_and_is_clamped() {
    let toml = "[[turn]]\ntext = \"a\"\n[[turn]]\ntext = \"b\"\n[[turn]]\ntext = \"c\"\n";
    let (session, store, mut rx) = open(toml, Config::default());
    // `Config::default()` carries no model catalogue, so nothing clamps here
    // (the router unit test covers that); `None` restores the tier's high.
    for (set, want) in [
        (Some(Effort::Low), Effort::Low),
        (Some(Effort::Xhigh), Effort::Xhigh),
        (None, Effort::High),
    ] {
        session
            .submit(Submission::SetEffort { effort: set })
            .await
            .expect("set effort");
        let running = spawn_turn(&session, "go");
        let events = drain(&mut rx).await;
        running.await.expect("join").expect("turn");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::StateChanged { effort, .. } if *effort == set))
        );
        let rows = store.usage_rows();
        assert_eq!(rows.last().expect("a usage row").effort, Some(want));
    }
}

#[tokio::test]
async fn set_mode_and_effort_each_emit_state_changed_with_both_values() {
    let (session, _store, mut rx) = open("[[turn]]\ntext = \"a\"\n", Config::default());
    session
        .submit(Submission::SetPermissionMode {
            mode: PermissionMode::Plan,
        })
        .await
        .expect("set mode");
    session
        .submit(Submission::SetEffort {
            effort: Some(Effort::Low),
        })
        .await
        .expect("set effort");
    let running = spawn_turn(&session, "go");
    let events = drain(&mut rx).await;
    running.await.expect("join").expect("turn");
    let changed: Vec<&Event> = events
        .iter()
        .filter(|e| matches!(e, Event::StateChanged { .. }))
        .collect();
    assert_eq!(
        changed,
        [
            &Event::StateChanged {
                mode: PermissionMode::Plan,
                effort: None,
            },
            &Event::StateChanged {
                mode: PermissionMode::Plan,
                effort: Some(Effort::Low),
            },
        ]
    );
    assert!(
        !events.iter().any(|e| matches!(e, Event::Notice { .. })),
        "the change is not echoed as a notice"
    );
}
