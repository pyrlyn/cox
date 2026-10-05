// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Re-read dedup through the loop (T2.6): a repeated read-only call shows a
//! pointer, a write to its subject brings the payload back.

mod common;

use common::{run_with, tool_results};
use cox_protocol::types::{Event, PermissionMode};

/// Writes run without a prompt so the scenario never waits on `Approve`.
fn auto(dedup_window_turns: u32) -> cox_protocol::Config {
    let mut config = cox_protocol::Config::default();
    config.permissions.mode = PermissionMode::Auto;
    config.context.dedup_window_turns = dedup_window_turns;
    config
}

fn archive_ids(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::ToolCallDone { result, .. } => Some(
                result
                    .archive
                    .as_ref()
                    .map(|a| a.id.to_string())
                    .unwrap_or_default(),
            ),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn dedup_second_identical_read_costs_under_50_tokens() {
    let (events, _, _) = run_with("reread", auto(8)).await;
    let results = tool_results(&events);
    assert_eq!(results.len(), 2);
    assert_eq!(results[0], (true, "a".to_string()));
    let (ok, second) = &results[1];
    assert!(*ok);
    let first_id = &archive_ids(&events)[0];
    assert_eq!(
        second,
        &format!("unchanged since turn 1, see #{first_id} (expand to re-show)")
    );
    // ~4 bytes per token: a pointer stays well under 50 tokens.
    assert!(second.len() < 200, "{second}");
}

#[tokio::test]
async fn dedup_write_invalidates_dedup() {
    let (events, _, _) = run_with("reread_after_write", auto(8)).await;
    let results = tool_results(&events);
    assert_eq!(results.len(), 5, "{results:?}");
    assert_eq!(results[0].1, "a");
    assert_eq!(results[1].1, "touched");
    assert!(
        results[2].1.starts_with("unchanged since turn 1"),
        "a write to b keeps the read of a: {}",
        results[2].1
    );
    assert_eq!(results[3].1, "touched");
    assert_eq!(results[4].1, "a", "a write to a brings the payload back");
}

#[tokio::test]
async fn dedup_window_zero_disables_dedup() {
    let (events, _, _) = run_with("reread", auto(0)).await;
    let visible: Vec<_> = tool_results(&events).into_iter().map(|r| r.1).collect();
    assert_eq!(visible, ["a", "a"]);
}
