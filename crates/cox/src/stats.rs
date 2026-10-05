// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox stats` (T1.7 per-session table, T8.3 `--cache`, T8.4 day/month
//! groupings by tier and job, context-token-turns, top tools, `--json` and
//! `--csv`). Pure summaries are built by `summarize_session` and rendered by
//! `render_json`/`render_csv_*` so tests never parse printed tables.

use std::collections::BTreeMap;
use std::path::Path;

use cox_protocol::{Event, SessionId, Store as _, UsageRow};
use cox_store::Store;
use cox_store::queries::{Period, TierJobRow, ToolBytesRow};
use serde::Serialize;

use crate::cli::StatsArgs;

/// The `serde` string form of a unit-variant enum (`"main"`, `"code"`).
fn tag<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Effort tag, or `-` for a row the ledger recorded before effort was
/// stored — never guessed into a default (`plan.md` A13).
pub fn effort_tag(effort: &Option<cox_protocol::types::Effort>) -> String {
    effort.as_ref().map(tag).unwrap_or_else(|| "-".into())
}

/// One `(tier, job)` total: the grouping every stats view reports.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TierJobTotal {
    /// Tier tag (`cheap`, `code`, `think`).
    pub tier: String,
    /// Job tag (`main`, `compact`, …).
    pub job: String,
    /// Provider calls aggregated.
    pub calls: u64,
    /// Summed usage.
    pub input_tokens: u64,
    /// Summed usage.
    pub output_tokens: u64,
    /// Summed usage.
    pub cache_read_tokens: u64,
    /// Summed usage.
    pub cache_write_tokens: u64,
    /// Summed usage (§1.9 context-token-turns).
    pub context_tokens: u64,
    /// Summed cost.
    pub cost_usd: f64,
}

/// A whole session in numbers: per-turn rows stay in the ledger, this is
/// the grouped view plus the `context-token-turns` total (§1.9).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionSummary {
    /// Session id.
    pub session: String,
    /// Provider calls.
    pub turns: u32,
    /// Sum of `context_tokens` over every call (§1.9).
    pub context_token_turns: u64,
    /// Summed cost.
    pub cost_usd: f64,
    /// Grouped by tier and job.
    pub by_tier_job: Vec<TierJobTotal>,
}

/// Groups `rows` by `(tier, job)` in tag order.
pub fn summarize_session(session: &SessionId, rows: &[UsageRow]) -> SessionSummary {
    let mut groups: BTreeMap<(String, String), TierJobTotal> = BTreeMap::new();
    for row in rows {
        let key = (tag(&row.tier), tag(&row.job));
        let total = groups.entry(key.clone()).or_insert(TierJobTotal {
            tier: key.0,
            job: key.1,
            calls: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            context_tokens: 0,
            cost_usd: 0.0,
        });
        total.calls += 1;
        total.input_tokens += u64::from(row.usage.input_tokens);
        total.output_tokens += u64::from(row.usage.output_tokens);
        total.cache_read_tokens += u64::from(row.usage.cache_read_tokens);
        total.cache_write_tokens += u64::from(row.usage.cache_write_tokens);
        total.context_tokens += u64::from(row.usage.context_tokens());
        total.cost_usd += row.usage.cost_usd;
    }
    let by_tier_job: Vec<TierJobTotal> = groups.into_values().collect();
    SessionSummary {
        session: session.to_string(),
        turns: rows.len() as u32,
        context_token_turns: by_tier_job.iter().map(|t| t.context_tokens).sum(),
        cost_usd: by_tier_job.iter().map(|t| t.cost_usd).sum(),
        by_tier_job,
    }
}

/// Pretty JSON of any summary view (the `--json` schema T8.4 snapshots).
pub fn render_json(value: &impl Serialize) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".into())
}

/// CSV of one session's per-turn rows.
pub fn render_csv_session(rows: &[UsageRow]) -> String {
    let mut out = String::from(
        "turn,job,tier,model,effort,input,output,cache_read,cache_write,cost_usd,latency_ms\n",
    );
    for row in rows {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{:.4},{}\n",
            row.turn,
            tag(&row.job),
            tag(&row.tier),
            row.model.0,
            effort_tag(&row.effort),
            row.usage.input_tokens,
            row.usage.output_tokens,
            row.usage.cache_read_tokens,
            row.usage.cache_write_tokens,
            row.usage.cost_usd,
            row.usage.latency_ms,
        ));
    }
    out
}

/// CSV of period/tier/job aggregates.
pub fn render_csv_periods(rows: &[TierJobRow]) -> String {
    let mut out = String::from(
        "period,tier,job,calls,input,output,cache_read,cache_write,context,cost_usd\n",
    );
    for row in rows {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{:.4}\n",
            row.period,
            row.tier,
            row.job,
            row.calls,
            row.input_tokens,
            row.output_tokens,
            row.cache_read_tokens,
            row.cache_write_tokens,
            row.context_tokens,
            row.cost_usd,
        ));
    }
    out
}

/// `cox stats` dispatch over the scope flags (§1.12).
pub fn run(home: &Path, args: &StatsArgs) -> anyhow::Result<()> {
    if args.json && args.csv {
        anyhow::bail!("--json and --csv are mutually exclusive");
    }
    if args.session.is_some() && (args.day || args.month || args.project.is_some()) {
        anyhow::bail!("--session cannot be combined with --day, --month or --project");
    }
    if args.project.is_some() && (args.day || args.month || args.cache) {
        anyhow::bail!("--project cannot be combined with --day, --month or --cache");
    }
    let store = Store::open(home)?;
    if let Some(project) = &args.project {
        return run_project(&store, project.as_deref(), args);
    }
    if let Some(session_id) = &args.session {
        let session: SessionId = session_id.parse()?;
        let rows = store.usage_for_session(&session)?;
        if rows.is_empty() {
            println!("No usage records found for this session");
            return Ok(());
        }
        if args.cache {
            return run_cache(&store, &session, &rows, args);
        }
        run_session(&store, &session, &rows, args)
    } else if args.day {
        run_periods(&store, Period::Day, args)
    } else if args.month {
        run_periods(&store, Period::Month, args)
    } else {
        run_periods(&store, Period::All, args)
    }
}

fn run_session(
    store: &Store,
    session: &SessionId,
    rows: &[UsageRow],
    args: &StatsArgs,
) -> anyhow::Result<()> {
    let summary = summarize_session(session, rows);
    let tools = store.top_tools(Some(session), 10).unwrap_or_default();
    if args.json {
        println!(
            "{}",
            render_json(&serde_json::json!({
                "session": summary,
                "top_tools": tools.iter().map(|t| serde_json::json!({
                    "tool": t.tool, "bytes": t.bytes, "calls": t.calls,
                })).collect::<Vec<_>>(),
            }))
        );
        return Ok(());
    }
    if args.csv {
        print!("{}", render_csv_session(rows));
        return Ok(());
    }
    // Print a header.
    println!(
        "{:<5} {:<20} {:<7} {:<12} {:<12} {:<12} {:<12} {:<12} {:<10}",
        "Turn", "Model", "Effort", "Input", "Output", "Cache R", "Cache W", "Cost", "Latency"
    );
    println!("{}", "-".repeat(113));

    // Print each row.
    for row in rows {
        println!(
            "{:<5} {:<20} {:<7} {:<12} {:<12} {:<12} {:<12} ${:<11.4} {:<10}ms",
            row.turn,
            row.model.0,
            effort_tag(&row.effort),
            row.usage.input_tokens,
            row.usage.output_tokens,
            row.usage.cache_read_tokens,
            row.usage.cache_write_tokens,
            row.usage.cost_usd,
            row.usage.latency_ms,
        );
    }

    // Print a summary line.
    let total_cost: f64 = rows.iter().map(|r| r.usage.cost_usd).sum();
    let total_input: u32 = rows.iter().map(|r| r.usage.input_tokens).sum();
    let total_output: u32 = rows.iter().map(|r| r.usage.output_tokens).sum();
    let total_cache_read: u32 = rows.iter().map(|r| r.usage.cache_read_tokens).sum();
    let total_cache_write: u32 = rows.iter().map(|r| r.usage.cache_write_tokens).sum();

    println!("{}", "-".repeat(113));
    println!(
        "{:<5} {:<20} {:<7} {:<12} {:<12} {:<12} {:<12} ${:<11.4}",
        "TOTAL", "", "", total_input, total_output, total_cache_read, total_cache_write, total_cost,
    );
    println!("context-token-turns: {}", summary.context_token_turns);
    print_tier_job(&summary.by_tier_job);
    print_top_tools(&tools);
    Ok(())
}

/// `cox stats --project [slug]` (T28.2): per-project totals from one SQL
/// aggregate each. `None` lists every project with spend.
fn run_project(store: &Store, slug: Option<&str>, args: &StatsArgs) -> anyhow::Result<()> {
    use cox_store::queries::ProjectTotals;
    let slugs: Vec<String> = match slug {
        Some(slug) => vec![slug.to_string()],
        None => store.project_slugs()?,
    };
    let mut totals: Vec<(String, ProjectTotals)> = Vec::with_capacity(slugs.len());
    for slug in slugs {
        totals.push((slug.clone(), store.project_totals(&slug)?));
    }
    totals.retain(|(_, t)| t.turns > 0);
    if totals.is_empty() {
        println!("No usage records found");
        return Ok(());
    }
    if args.json {
        println!(
            "{}",
            render_json(&serde_json::json!(
                totals
                    .iter()
                    .map(|(slug, t)| serde_json::json!({
                        "project": slug, "sessions": t.sessions, "turns": t.turns,
                        "cost_usd": t.cost_usd, "tokens": t.tokens,
                    }))
                    .collect::<Vec<_>>()
            ))
        );
        return Ok(());
    }
    if args.csv {
        let mut out = String::from("project,sessions,turns,cost_usd,tokens\n");
        for (slug, t) in &totals {
            out.push_str(&format!(
                "{},{},{},{:.4},{}\n",
                slug, t.sessions, t.turns, t.cost_usd, t.tokens,
            ));
        }
        print!("{out}");
        return Ok(());
    }
    println!(
        "{:<24} {:<8} {:<7} {:<12} {:<12}",
        "Project", "Sessions", "Turns", "Cost", "Tokens"
    );
    println!("{}", "-".repeat(70));
    for (slug, t) in &totals {
        println!(
            "{:<24} {:<8} {:<7} ${:<11.4} {:<12}",
            slug, t.sessions, t.turns, t.cost_usd, t.tokens,
        );
    }
    Ok(())
}

fn run_periods(store: &Store, period: Period, args: &StatsArgs) -> anyhow::Result<()> {
    let rows = store.usage_by_period(period)?;
    if rows.is_empty() {
        println!("No usage records found");
        return Ok(());
    }
    if args.json {
        println!("{}", render_json(&rows_as_json(&rows)));
        return Ok(());
    }
    if args.csv {
        print!("{}", render_csv_periods(&rows));
        return Ok(());
    }
    println!(
        "{:<10} {:<8} {:<10} {:<7} {:<12} {:<12} {:<12}",
        "Period", "Tier", "Job", "Calls", "Input", "Cache R", "Cost"
    );
    println!("{}", "-".repeat(80));
    for row in &rows {
        println!(
            "{:<10} {:<8} {:<10} {:<7} {:<12} {:<12} ${:<11.4}",
            row.period,
            row.tier,
            row.job,
            row.calls,
            row.input_tokens,
            row.cache_read_tokens,
            row.cost_usd,
        );
    }
    let total: f64 = rows.iter().map(|r| r.cost_usd).sum();
    let context: i64 = rows.iter().map(|r| r.context_tokens).sum();
    println!("{}", "-".repeat(80));
    println!("TOTAL ${total:.4} · context-token-turns: {context}");
    if period == Period::All {
        print_top_tools(&store.top_tools(None, 10).unwrap_or_default());
    }
    Ok(())
}

fn rows_as_json(rows: &[TierJobRow]) -> serde_json::Value {
    serde_json::json!(
        rows.iter()
            .map(|r| serde_json::json!({
                "period": r.period, "tier": r.tier, "job": r.job,
                "calls": r.calls, "input_tokens": r.input_tokens,
                "output_tokens": r.output_tokens,
                "cache_read_tokens": r.cache_read_tokens,
                "cache_write_tokens": r.cache_write_tokens,
                "context_tokens": r.context_tokens, "cost_usd": r.cost_usd,
            }))
            .collect::<Vec<_>>()
    )
}

fn print_tier_job(totals: &[TierJobTotal]) {
    println!("by tier/job:");
    for t in totals {
        println!(
            "  {:<8} {:<10} {:<5} calls ${:<11.4}",
            t.tier, t.job, t.calls, t.cost_usd,
        );
    }
}

fn print_top_tools(tools: &[ToolBytesRow]) {
    if tools.is_empty() {
        return;
    }
    println!("top tools by archived bytes:");
    for t in tools {
        println!("  {:<24} {:>8} bytes {:>4} calls", t.tool, t.bytes, t.calls);
    }
}

/// `cox stats --cache`: per-turn read ratio plus the miss `Notice`s the core
/// emitted (T8.3 step 3). A turn with ratio 0 after a non-zero one is the
/// ledger side of the same miss the rollout names by block.
fn run_cache(
    store: &Store,
    session: &SessionId,
    rows: &[UsageRow],
    args: &StatsArgs,
) -> anyhow::Result<()> {
    if args.json {
        println!(
            "{}",
            render_json(&serde_json::json!(
                rows.iter()
                    .map(|row| serde_json::json!({
                        "turn": row.turn,
                        "cache_ratio": cox_core::cache_diag::ratio_of(&row.usage),
                        "input_tokens": row.usage.input_tokens,
                        "cache_read_tokens": row.usage.cache_read_tokens,
                        "cache_write_tokens": row.usage.cache_write_tokens,
                    }))
                    .collect::<Vec<_>>()
            ))
        );
        return Ok(());
    }
    if args.csv {
        let mut out = String::from("turn,cache_ratio,input,read,write\n");
        for row in rows {
            out.push_str(&format!(
                "{},{:.2},{},{},{}\n",
                row.turn,
                cox_core::cache_diag::ratio_of(&row.usage),
                row.usage.input_tokens,
                row.usage.cache_read_tokens,
                row.usage.cache_write_tokens,
            ));
        }
        print!("{out}");
        return Ok(());
    }
    println!(
        "{:<5} {:<8} {:<12} {:<12} {:<12}",
        "Turn", "Cache", "Input", "Read", "Write"
    );
    println!("{}", "-".repeat(55));
    for row in rows {
        let ratio = cox_core::cache_diag::ratio_of(&row.usage);
        println!(
            "{:<5} {:<8} {:<12} {:<12} {:<12}",
            row.turn,
            format!("{}%", (ratio * 100.0).round() as u64),
            row.usage.input_tokens,
            row.usage.cache_read_tokens,
            row.usage.cache_write_tokens,
        );
    }
    match store.rollout_read(session) {
        Ok(events) => {
            let misses: Vec<&str> = events
                .iter()
                .filter_map(|e| match e {
                    Event::Notice { text, .. } if text.starts_with("cache miss") => {
                        Some(text.as_str())
                    }
                    _ => None,
                })
                .collect();
            if misses.is_empty() {
                println!("no cache misses recorded");
            } else {
                for m in misses {
                    println!("{m}");
                }
            }
        }
        Err(e) => println!("rollout unavailable: {e}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use cox_protocol::{CallId, Job, ModelId, ProviderId, Tier, Usage};

    use super::*;

    fn row(job: Job, tier: Tier, turn: u32, input: u32, cost: f64) -> UsageRow {
        UsageRow {
            session_id: SessionId::new(),
            turn,
            job,
            tier,
            provider: ProviderId::Anthropic,
            model: ModelId("claude-sonnet-5".into()),
            effort: Some(cox_protocol::types::Effort::High),
            usage: Usage {
                input_tokens: input,
                output_tokens: 10,
                cache_read_tokens: 90,
                cache_write_tokens: 0,
                estimated: false,
                cost_usd: cost,
                latency_ms: 100,
            },
        }
    }

    #[test]
    fn stats_session_summary_groups_by_tier_and_job() {
        let session = SessionId::new();
        let rows = vec![
            row(Job::Main, Tier::Code, 1, 100, 0.01),
            row(Job::Main, Tier::Code, 2, 200, 0.02),
            row(Job::Compact, Tier::Cheap, 3, 50, 0.001),
        ];
        let summary = summarize_session(&session, &rows);
        assert_eq!(summary.turns, 3);
        assert_eq!(summary.by_tier_job.len(), 2);
        let main = summary
            .by_tier_job
            .iter()
            .find(|t| t.job == "main")
            .expect("main group");
        assert_eq!((main.tier.as_str(), main.calls), ("code", 2));
        // context = input + read + write per call: (100+90) + (200+90) + (50+90).
        assert_eq!(summary.context_token_turns, 190 + 290 + 140);
        assert!((summary.cost_usd - 0.031).abs() < 1e-9);
    }

    #[test]
    fn stats_json_holds_the_summary_shape() {
        let session = SessionId::new();
        let summary = summarize_session(&session, &[row(Job::Main, Tier::Code, 1, 100, 0.01)]);
        let value: serde_json::Value = serde_json::from_str(&render_json(&summary)).expect("json");
        assert_eq!(value["turns"], 1);
        assert_eq!(value["context_token_turns"], 190);
        assert_eq!(value["by_tier_job"][0]["tier"], "code");
    }

    #[test]
    fn stats_csv_starts_with_a_header_row() {
        let out = render_csv_session(&[row(Job::Main, Tier::Code, 1, 100, 0.01)]);
        let mut lines = out.lines();
        assert_eq!(
            lines.next().unwrap(),
            "turn,job,tier,model,effort,input,output,cache_read,cache_write,cost_usd,latency_ms"
        );
        // The effort column carries the recorded tag, not a guessed default.
        assert_eq!(lines.next().unwrap().split(',').nth(4), Some("high"));
        assert_eq!(lines.count(), 0);
        let periods = render_csv_periods(&[]);
        assert!(periods.starts_with("period,tier,job,calls,"));
    }

    #[test]
    fn stats_top_tools_orders_by_bytes() {
        use cox_protocol::{ArchivePut, Store as _};
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path()).expect("open");
        let session = SessionId::new();
        for (tool, byte) in [("read", 5u8), ("bash", 100u8)] {
            store
                .archive_put(&ArchivePut {
                    session,
                    call: CallId::new(),
                    tool: tool.into(),
                    subject: None,
                    bytes: vec![byte; if tool == "bash" { 100 } else { 5 }],
                })
                .expect("put");
        }
        let top = store.top_tools(None, 10).expect("top");
        assert_eq!(top.len(), 2);
        assert_eq!((top[0].tool.as_str(), top[0].bytes), ("bash", 100));
    }

    #[test]
    fn stats_usage_by_period_buckets_one_day() {
        use cox_protocol::{SessionRow, Store as _};
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path()).expect("open");
        let session = SessionId::new();
        store
            .session_create(&SessionRow {
                id: session,
                created_at: String::new(),
                cwd: std::path::PathBuf::from("/tmp"),
                project_slug: "cox".into(),
                title: None,
                parent_id: None,
                rollout_path: dir.path().join("sessions").join(format!("{session}.jsonl")),
            })
            .expect("session");
        let mut first = row(Job::Main, Tier::Code, 1, 100, 0.01);
        first.session_id = session;
        let mut second = row(Job::Compact, Tier::Cheap, 2, 50, 0.001);
        second.session_id = session;
        store.usage_insert(&first).expect("insert 1");
        store.usage_insert(&second).expect("insert 2");
        let days = store.usage_by_period(Period::Day).expect("days");
        assert_eq!(days.len(), 2, "one bucket per (tier, job)");
        assert!(days.iter().all(|r| r.period.len() == 10), "{days:?}");
        let months = store.usage_by_period(Period::Month).expect("months");
        assert!(months.iter().all(|r| r.period.len() == 7), "{months:?}");
        let all = store.usage_by_period(Period::All).expect("all");
        assert!(all.iter().all(|r| r.period == "all"), "{all:?}");
        assert_eq!(all.iter().map(|r| r.calls).sum::<i64>(), 2);
    }
}
