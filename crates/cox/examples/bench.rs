// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Token-economy bench (T8.5): replays the recorded transcripts in
//! `evals/token/sessions` through the real `Session` loop — `Scripted`
//! provider, real `read`/`grep`/`glob` tools over `evals/token/workspace` —
//! once per D6 mechanism with that mechanism disabled, and once as the
//! shipped-defaults baseline. Context-token-turns per run come from the
//! ledger rows the loop itself wrote (`Usage::context_tokens`), so every
//! number below passes through production code paths: truncation, dedup,
//! microcompaction, compaction, deferred schemas and the token heuristic.
//!
//! `prefix` is the exception: an offline replay cannot observe server cache
//! hits, so it counts the stable-prefix bytes an unstable order would force
//! the server to re-receive on every call after the first (real `assemble`
//! output, real `estimate`). See `evals/token/README.md`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cox_core::{MemoryStore, Session};
use cox_protocol::Config;
use cox_protocol::traits::Tool;
use cox_protocol::types::{Effort, Event, Job, ModelId, Request, Submission, Thinking, Tier};

#[derive(Debug, serde::Deserialize)]
struct Call {
    tool: String,
    input: HashMap<String, String>,
}

#[derive(Debug, serde::Deserialize)]
struct Turn {
    user: String,
    #[serde(default)]
    assistant: String,
    #[serde(default)]
    calls: Vec<Call>,
    #[serde(default)]
    #[serde(rename = "final")]
    final_text: String,
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// One transcript's provider calls as Scripted TOML. `outline_off` rewrites
/// `read` calls from `outline` to `text`; `with_summary_after == Some(n)`
/// splices the summary spec after the nth turn's calls (baseline only).
fn specs(
    turns: &[Turn],
    summary: &str,
    outline_off: bool,
    with_summary_after: Option<usize>,
) -> String {
    let mut out = String::new();
    for (i, turn) in turns.iter().enumerate() {
        if turn.calls.is_empty() {
            let text = if turn.final_text.is_empty() {
                &turn.assistant
            } else {
                &turn.final_text
            };
            out.push_str(&format!("[[turn]]\ntext = \"{}\"\n", esc(text)));
        } else {
            out.push_str(&format!("[[turn]]\ntext = \"{}\"\n", esc(&turn.assistant)));
            for call in &turn.calls {
                out.push_str(&format!(
                    "[[turn.tool_calls]]\nname = \"{}\"\n",
                    esc(&call.tool)
                ));
                let input = call
                    .input
                    .iter()
                    .map(|(k, v)| {
                        let v = if outline_off && call.tool == "read" && k == "mode" {
                            "text".to_string()
                        } else {
                            v.clone()
                        };
                        format!("{k} = \"{}\"", esc(&v))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push_str(&format!("input = {{ {input} }}\n"));
            }
            out.push_str(&format!("[[turn]]\ntext = \"{}\"\n", esc(&turn.final_text)));
        }
        if with_summary_after == Some(i) {
            out.push_str(&format!("[[turn]]\ntext = \"{}\"\n", esc(summary)));
        }
    }
    out
}

fn expected_calls(turns: &[Turn], summary: bool) -> usize {
    let mut n = 0;
    for turn in turns {
        n += if turn.calls.is_empty() { 1 } else { 2 };
    }
    n + usize::from(summary)
}

fn tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(cox_tools::read::ReadTool),
        Arc::new(cox_tools::grep::GrepTool),
        Arc::new(cox_tools::glob::GlobTool),
        Arc::new(cox_tools::web_fetch::WebFetchTool::new()),
        Arc::new(cox_tools::ask_user::AskUserTool::new(
            cox_tools::ask_user::Answers::Fixed(None),
        )),
    ]
}

async fn drain(rx: &mut tokio::sync::mpsc::Receiver<Event>) {
    loop {
        match rx.recv().await {
            Some(Event::TurnDone { .. }) => break,
            Some(_) => {}
            None => break,
        }
    }
}

/// Replays one transcript; returns context-token-turns and provider calls.
async fn replay(
    turns: &[Turn],
    summary: &str,
    ws: &Path,
    variant: &str,
    compact_after: Option<usize>,
) -> (u64, usize) {
    let mut config = Config::default();
    config.core.workspace_roots = vec![ws.to_path_buf()];
    match variant {
        "archive-off" => {
            config.context.tool_output_visible_bytes = u32::MAX;
            config.context.tool_output_head_lines = 1_000_000;
            config.context.tool_output_tail_lines = 1_000_000;
        }
        "dedup-off" => config.context.dedup_window_turns = 0,
        "deferred-off" => config.context.deferred_tools = false,
        "compaction-off" => {
            config.context.compact_at = 1.0;
            config.context.microcompact_after_turns = u32::MAX;
        }
        _ => {}
    }
    let outline_off = variant == "outline-off";
    let toml = specs(turns, summary, outline_off, compact_after);
    let expect = expected_calls(turns, compact_after.is_some());
    let provider =
        Arc::new(cox_provider::scripted::Scripted::from_toml(&toml, "").expect("scenario"));
    let store = Arc::new(MemoryStore::new());
    let session = Session::new(
        config,
        provider,
        tools(),
        store.clone(),
        store.clone(),
        ws.to_path_buf(),
    )
    .expect("session");
    let mut rx = session.events().expect("events");
    for (i, turn) in turns.iter().enumerate() {
        session
            .submit(Submission::UserTurn {
                text: turn.user.clone(),
                attachments: vec![],
                confirm_think: false,
            })
            .await
            .expect("turn");
        drain(&mut rx).await;
        if compact_after == Some(i) {
            session
                .submit(Submission::Compact { focus: None })
                .await
                .expect("compact");
        }
    }
    let rows = store.usage_rows();
    if rows.len() != expect {
        eprintln!(
            "warn: {variant}: expected {expect} provider calls, got {}",
            rows.len()
        );
    }
    let total = rows
        .iter()
        .map(|r| u64::from(r.usage.context_tokens()))
        .sum();
    (total, rows.len())
}

/// Emulated prefix cost: stable-prefix bytes an unstable tool order would
/// force the server to re-receive on every call after the first.
fn prefix_rewrite_tokens(ws: &Path) -> u64 {
    let config = Config::default();
    let history = vec![cox_protocol::types::Message {
        role: cox_protocol::types::Role::User,
        content: vec![cox_protocol::types::Content::Text {
            text: "hello".into(),
        }],
    }];
    let full = cox_core::assemble(&history, &config, &tools(), ws, "");
    let bare = Request {
        tier: Tier::Code,
        job: Job::Main,
        model: ModelId("bench".into()),
        system: vec![],
        tools: vec![],
        messages: full.messages.clone(),
        effort: Effort::Low,
        max_tokens: 1024,
        thinking: Thinking::Off,
        cache_breakpoints: vec![],
        stop_sequences: vec![],
    };
    cox_provider::tokens::estimate(&full)
        .tokens
        .saturating_sub(cox_provider::tokens::estimate(&bare).tokens) as u64
}

#[tokio::main]
async fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let evals = root.join("..").join("..").join("evals").join("token");
    let ws = evals.join("workspace");
    let sessions_dir = evals.join("sessions");
    let mut names: Vec<String> = std::fs::read_dir(&sessions_dir)
        .expect("sessions dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".jsonl"))
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no recorded sessions");

    // Transcript name -> (turns, summary).
    let mut transcripts = Vec::new();
    for name in &names {
        let text = std::fs::read_to_string(sessions_dir.join(name)).expect("read session");
        let mut lines = text.lines();
        let summary: String = serde_json::from_str(lines.next().unwrap_or(""))
            .ok()
            .and_then(|v: serde_json::Value| {
                v.get("summary")
                    .and_then(|s| s.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let turns: Vec<Turn> = lines
            .map(|l| serde_json::from_str(l).expect("turn json"))
            .collect();
        transcripts.push((name.clone(), turns, summary));
    }

    let variants = [
        "archive-off",
        "dedup-off",
        "outline-off",
        "deferred-off",
        "compaction-off",
    ];
    let mut base_total = 0u64;
    let mut off_totals: HashMap<&str, u64> = HashMap::new();
    let mut total_calls = 0usize;
    for (name, turns, summary) in &transcripts {
        let (base, calls) = replay(turns, summary, &ws, "baseline", Some(3)).await;
        base_total += base;
        total_calls += calls;
        for v in &variants {
            let compact = if *v == "compaction-off" {
                None
            } else {
                Some(3)
            };
            let (total, _) = replay(turns, summary, &ws, v, compact).await;
            *off_totals.entry(v).or_insert(0) += total;
        }
        eprintln!("replayed {name}: baseline {base} context tokens");
    }

    // Prefix emulation on top of the measured baseline.
    let rewrite = prefix_rewrite_tokens(&ws);
    let prefix_before = base_total + rewrite * total_calls.saturating_sub(1) as u64;

    println!("| mechanism | sessions | context-token-turns before | after | Δ |");
    println!("|---|---|---|---|---|");
    let mut any_nonzero = false;
    for v in ["archive", "dedup", "outline", "deferred", "compaction"] {
        let key = format!("{v}-off");
        let before = off_totals.get(key.as_str()).copied().unwrap_or(0);
        let pct = if before == 0 {
            0.0
        } else {
            (before.saturating_sub(base_total)) as f64 / before as f64 * 100.0
        };
        any_nonzero |= pct > 0.0;
        println!(
            "| {v} | {} | {before} | {base_total} | {pct:.1} % |",
            names.len()
        );
    }
    let prefix_pct = if prefix_before == 0 {
        0.0
    } else {
        (prefix_before - base_total) as f64 / prefix_before as f64 * 100.0
    };
    any_nonzero |= prefix_pct > 0.0;
    println!(
        "| prefix | {} | {prefix_before} | {base_total} | {prefix_pct:.1} % |",
        names.len()
    );
    assert!(any_nonzero, "bench measured no savings anywhere");
}
