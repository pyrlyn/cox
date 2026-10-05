// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The exported surface end to end (T37.14): a real session in a scratch
//! `COX_HOME`, driven through `App` and `SessionHandle` as Swift drives
//! them, with the in-memory host from the fixture recorder. The session
//! logic itself is `cox-app`'s and tested there (T37.39); these prove the
//! forwarding, the runtime, the host bridge and the error mapping. nextest
//! runs each test in its own process, so each sets its own environment.

// why: env::set_var/remove_var are unsafe in edition 2024 (per-process tests).
#![allow(unsafe_code)]

#[path = "../examples/record.rs"]
#[allow(dead_code)]
mod record;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cox_app::BlockKind;
use cox_ffi::AppError;
use cox_protocol::types::StopReason;
use record::{MemoryHost, open};

fn scenario(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("fixtures/{name}.toml"))
}

fn scratch(scenario: Option<&Path>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    // SAFETY: first thing in this test's own process (nextest).
    unsafe { record::scratch_env(dir.path(), scenario) };
    dir
}

#[tokio::test]
async fn a_scripted_turn_reaches_the_app_as_blocks_and_the_meter() {
    let dir = scratch(Some(&scenario("read-and-reply")));
    let host = Arc::new(MemoryHost::default());
    let session = open(dir.path(), Arc::clone(&host)).await.expect("open");
    let prompts = ["read the notes".to_string()];
    let recording = record::record(&session, &host, &prompts)
        .await
        .expect("record");
    assert!(
        recording["batches"]
            .as_array()
            .is_some_and(|b| !b.is_empty())
    );

    let kinds: Vec<BlockKind> = session.snapshot().into_iter().map(|b| b.kind).collect();
    let user =
        |k: &BlockKind| matches!(k, BlockKind::User { text, .. } if text == "read the notes");
    assert!(kinds.iter().any(user), "{kinds:?}");
    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, BlockKind::Tool { tool, .. } if tool == "read"))
    );
    assert!(kinds.iter().any(
        |k| matches!(k, BlockKind::Assistant { text, doc, .. } if text.contains("**hello**") && !doc.blocks.is_empty())
    ));
    assert!(kinds.iter().any(|k| matches!(
        k,
        BlockKind::TurnMeta {
            stop: Some(StopReason::EndTurn),
            ..
        }
    )));
    let batches = &recording["batches"];
    let usage = batches.to_string().contains(r#""op":"usage""#);
    assert!(usage, "the meter's patch crosses too: {batches}");
}

#[tokio::test]
async fn the_host_keychain_supplies_the_provider_key_and_its_absence_fails() {
    let dir = scratch(None);
    let empty = Arc::new(MemoryHost::default());
    let err = open(dir.path(), Arc::clone(&empty)).await.err();
    assert!(matches!(err, Some(AppError::Session { .. })), "{err:?}");
    assert_eq!(*empty.asked.lock().expect("asked"), ["anthropic"]);

    let mut keyed = MemoryHost::default();
    keyed.secrets.insert("anthropic".into(), "sk-test".into());
    let session = open(dir.path(), Arc::new(keyed)).await;
    assert!(session.is_ok(), "{:?}", session.err());
}

#[tokio::test]
async fn an_approval_is_noted_with_badge_one_and_allowing_it_resumes_the_turn() {
    let dir = scratch(Some(&scenario("approve-write")));
    let host = Arc::new(MemoryHost::default());
    let session = open(dir.path(), Arc::clone(&host)).await.expect("open");
    let prompts = ["write a summary".to_string()];
    let recording = record::record(&session, &host, &prompts)
        .await
        .expect("record");

    let notes = recording["notes"].as_array().cloned().unwrap_or_default();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(notes[0]["badge"], 1);
    assert_eq!(notes[0]["item"]["need"]["type"], "approval");
    let kinds: Vec<BlockKind> = session.snapshot().into_iter().map(|b| b.kind).collect();
    let allowed = |k: &BlockKind| {
        matches!(
            k,
            BlockKind::Approval {
                decision: Some(_),
                ..
            }
        )
    };
    assert!(kinds.iter().any(allowed), "{kinds:?}");
    let ended = |k: &BlockKind| {
        matches!(
            k,
            BlockKind::TurnMeta {
                stop: Some(StopReason::EndTurn),
                ..
            }
        )
    };
    assert!(kinds.iter().any(ended), "{kinds:?}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("project/summary.md")).ok(),
        Some("hello\n".into())
    );
}

/// The `App` over `dir`'s scratch home, as Swift makes it.
fn ffi_app(dir: &Path, host: Arc<MemoryHost>) -> Arc<cox_ffi::App> {
    let home = dir.join("user/.cox").to_string_lossy().into_owned();
    cox_ffi::App::new(Some(home), host).expect("app")
}

#[test]
fn models_reach_swift_with_the_code_tiers_model_first() {
    let dir = scratch(None);
    let cwd = dir.path().to_string_lossy().into_owned();
    let models = ffi_app(dir.path(), Arc::default())
        .models(cwd)
        .expect("models");
    let first = models.first().expect("a model");
    assert_eq!(first.tier, cox_protocol::types::Tier::Code);
    assert_eq!(first.id, "claude-sonnet-5");
}

#[tokio::test]
async fn usable_providers_ask_the_host_keychain() {
    let dir = scratch(None);
    let mut keyed = MemoryHost::default();
    keyed.secrets.insert("anthropic".into(), "sk-test".into());
    let cwd = dir.path().to_string_lossy().into_owned();
    let usable = ffi_app(dir.path(), Arc::new(keyed))
        .usable_providers(cwd)
        .await
        .expect("providers");
    assert!(usable.contains(&"anthropic".to_string()), "{usable:?}");
}

#[tokio::test]
async fn workspace_changed_returns_once_a_session_commits() {
    let dir = scratch(Some(&scenario("read-and-reply")));
    let watching = tokio::spawn(ffi_app(dir.path(), Arc::default()).workspace_changed());
    // Let it take its position in the feed before anything commits.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let host = Arc::new(MemoryHost::default());
    let session = open(dir.path(), Arc::clone(&host)).await.expect("open");
    let prompts = ["read the notes".to_string()];
    record::record(&session, &host, &prompts)
        .await
        .expect("record");
    let woke = tokio::time::timeout(std::time::Duration::from_secs(20), watching).await;
    assert!(matches!(woke, Ok(Ok(Ok(())))), "{woke:?}");
}

#[tokio::test]
async fn output_of_an_unknown_archive_is_a_session_error() {
    let dir = scratch(Some(&scenario("read-and-reply")));
    let session = open(dir.path(), Arc::default()).await.expect("open");
    let err = session.output(cox_protocol::ids::ArchiveId::new()).err();
    assert!(matches!(err, Some(AppError::Session { .. })), "{err:?}");
}
