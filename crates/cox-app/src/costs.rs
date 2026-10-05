// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The Context & Cost tab's cost history (T37.29.3.2, DT§5.1, mockup 10's
//! "Cost by turn"): the session's ledger `usage` rows grouped by turn, each
//! turn's subagents right under it, and the session total, every figure
//! formatted. Built from the rows the store returns, never from the meter's
//! running sums, because a cost that is not a ledger row does not exist.
//! Also the tab's footnote, the project's spend today and this week
//! (T37.29.3.3), the budget caps and how close the spend is (T37.29.3.4),
//! and the menu bar's "Today" footer over the whole ledger
//! (T51.12). Separate from `meter_text.rs`, which formats the live meter.

use chrono::{DateTime, Datelike, Days, NaiveDate, SecondsFormat, TimeZone, Utc};
use cox_protocol::StoreError;
use cox_protocol::config::BudgetConfig;
use cox_protocol::types::{Job, Usage};
use cox_store::queries::LedgerRow;
use cox_store::{Store, to_tag};
use serde::Serialize;

use crate::meter_text::tokens;
use crate::usage::add_to;
use crate::workspace::{Workspace, WorkspaceError};

/// What the tab's "Cost by turn" grid shows.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct TurnCosts {
    /// The value columns' headers: `In`, `Out`, `Cache r/w`, `$`.
    pub columns: Vec<String>,
    /// A row per turn in order, each turn's subagents under it as detail
    /// rows; empty before the first provider call.
    pub rows: Vec<CostRow>,
    /// `Session`: every row above summed.
    pub total: CostRow,
    /// The footnote: `Project cox today: $3.18 · this week: $21.40. …`.
    pub project: String,
    /// `[budget]`'s two caps against what was spent, session first; see
    /// [`budget_rows`].
    pub budget: Vec<BudgetRow>,
}

/// One cap against its spend: `Session`, `$0.42 of $5.00`, a gauge at 8%.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct BudgetRow {
    pub label: String,
    /// `$0.42 of $5.00`, or `$0.42` alone when the cap is not a usable
    /// number.
    pub text: String,
    /// The spend over the cap, clamped to 0..=1 so the gauge never draws
    /// past its end; none without a usable cap.
    pub fraction: Option<f64>,
}

/// One line of the grid: `1 · code` (a subagent's `explore`), then a value
/// per column.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CostRow {
    pub label: String,
    pub values: Vec<String>,
    /// A subagent's row, drawn indented under its turn.
    pub detail: bool,
}

const COLUMNS: [&str; 4] = ["In", "Out", "Cache r/w", "$"];

/// One turn's rows so far.
struct Turn {
    label: String,
    /// When its first call was written; a subagent that starts later
    /// belongs to it until the next turn starts.
    start: String,
    sum: Option<Usage>,
    subagents: Vec<CostRow>,
}

/// `own` is the session's ledger in written order; `children` each child
/// session's, of which only subagents count (a fork or handoff is a session
/// of its own, not this one's cost). `month_usd` is the whole ledger's spend
/// since the month began, which `[budget].monthly_usd` caps.
pub fn build(
    own: &[LedgerRow],
    children: &[Vec<LedgerRow>],
    budget: &BudgetConfig,
    month_usd: f64,
) -> TurnCosts {
    let mut turns: Vec<Turn> = Vec::new();
    let mut total = None;
    let mut spent = 0.0;
    for row in own {
        let u = &row.usage;
        // `turn` restarts at 1 with every turn; a side call (compaction,
        // a summary) is 0 and belongs to the turn it ran in.
        if u.turn == 1 || turns.is_empty() {
            let n = turns.iter().filter(|t| !t.label.is_empty()).count() + 1;
            turns.push(Turn {
                label: if u.turn == 0 {
                    String::new()
                } else {
                    format!("{n} · {}", to_tag(&u.tier))
                },
                start: row.created_at.clone(),
                sum: None,
                subagents: Vec::new(),
            });
        }
        if let Some(turn) = turns.last_mut() {
            turn.sum = Some(add_to(turn.sum, &u.usage));
        }
        total = Some(add_to(total, &u.usage));
        spent += u.usage.cost_usd;
    }
    let mut orphans = Vec::new();
    for rows in children {
        let Some(first) = rows.first().filter(|r| is_subagent(&r.usage.job)) else {
            continue;
        };
        let mut sum = None;
        for r in rows {
            sum = Some(add_to(sum, &r.usage.usage));
            total = Some(add_to(total, &r.usage.usage));
            spent += r.usage.usage.cost_usd;
        }
        // The job alone: with its tier the label wraps in the inspector's
        // width, and a subagent's tier is its preset's.
        let line = row(to_tag(&first.usage.job), sum, true);
        match turns.iter().rposition(|t| t.start <= first.created_at) {
            Some(at) => turns[at].subagents.push(line),
            None => orphans.push(line),
        }
    }
    let mut rows = Vec::new();
    for turn in turns {
        let label = if turn.label.is_empty() {
            "Before turn 1".to_string()
        } else {
            turn.label
        };
        rows.push(row(label, turn.sum, false));
        rows.extend(turn.subagents);
    }
    rows.extend(orphans);
    TurnCosts {
        columns: COLUMNS.map(String::from).to_vec(),
        rows,
        total: row("Session".into(), total, false),
        project: String::new(),
        budget: budget_rows(budget, spent, month_usd),
    }
}

/// The session's and the month's spend against `[budget]`. `BudgetConfig`
/// has no "no cap" value, and the core stops at a cap of 0 or less, so a cap
/// that is not a positive number leaves its row with the spend alone rather
/// than a fraction of nothing.
pub fn budget_rows(budget: &BudgetConfig, session_usd: f64, month_usd: f64) -> Vec<BudgetRow> {
    let row = |label: &str, spent: f64, cap: f64| {
        let usable = cap.is_finite() && cap > 0.0;
        BudgetRow {
            label: label.into(),
            text: if usable {
                format!("${spent:.2} of ${cap:.2}")
            } else {
                format!("${spent:.2}")
            },
            fraction: usable.then(|| (spent / cap).clamp(0.0, 1.0)),
        }
    };
    vec![
        row("Session", session_usd, budget.session_usd),
        row("This month", month_usd, budget.monthly_usd),
    ]
}

/// `date`'s start in `tz`, written as `usage.created_at` is (UTC,
/// milliseconds) so the store compares it as text. A zone that skips midnight
/// for daylight saving starts the day an hour or two later.
fn local_start<Tz: TimeZone>(tz: &Tz, date: NaiveDate) -> Option<String> {
    (0..3).find_map(|h| {
        tz.from_local_datetime(&date.and_hms_opt(h, 0, 0)?)
            .earliest()
            .map(|d| {
                d.with_timezone(&Utc)
                    .to_rfc3339_opts(SecondsFormat::Millis, true)
            })
    })
}

/// The starts of `now`'s day and of its week (Monday, ISO 8601) in its own
/// time zone, as [`local_start`] writes them.
pub fn periods<Tz: TimeZone>(now: &DateTime<Tz>) -> (String, String) {
    let tz = now.timezone();
    let start = |date: NaiveDate| {
        local_start(&tz, date).unwrap_or_else(|| {
            now.with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Millis, true)
        })
    };
    let today = now.date_naive();
    let back = Days::new(u64::from(today.weekday().num_days_from_monday()));
    (start(today), start(today - back))
}

/// The start of `now`'s calendar month in its own time zone.
pub fn month_start<Tz: TimeZone>(now: &DateTime<Tz>) -> String {
    let first = now
        .date_naive()
        .with_day(1)
        .unwrap_or_else(|| now.date_naive());
    local_start(&now.timezone(), first).unwrap_or_else(|| {
        now.with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true)
    })
}

/// What the whole ledger spent since `now`'s month began: the figure
/// `[budget].monthly_usd` caps, across every project.
pub fn month_spend<Tz: TimeZone>(store: &Store, now: &DateTime<Tz>) -> Result<f64, StoreError> {
    Ok(store.activity_since(&month_start(now))?.0)
}

/// The tab's footnote over the project's spend (mockup 10).
pub fn footnote(project: &str, today: f64, week: f64) -> String {
    format!(
        "Project {project} today: ${today:.2} · this week: ${week:.2}. \
         Every number is a row in the cost ledger."
    )
}

/// The menu bar's "Today" footer (T51.12, mockup 26), every figure a
/// ledger row's.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DaySummary {
    /// `$3.18`: the `usage` rows written since local midnight, formatted as
    /// the meter's cost.
    pub cost: String,
    /// The sessions written to since local midnight.
    pub sessions: u64,
    /// The footer's figures after its "Today" label: `$3.18 · 4 sessions`.
    pub text: String,
}

impl Workspace {
    /// Today's spend and sessions across every project, for the menu bar.
    pub fn today(&self) -> Result<DaySummary, WorkspaceError> {
        Ok(day_summary(self.store(), &chrono::Local::now())?)
    }
}

/// `now`'s day from its local midnight, as [`periods`] starts it.
fn day_summary<Tz: TimeZone>(store: &Store, now: &DateTime<Tz>) -> Result<DaySummary, StoreError> {
    let (today, _) = periods(now);
    let (spent, active) = store.activity_since(&today)?;
    let sessions = u64::try_from(active).unwrap_or(0);
    let cost = format!("${spent:.2}");
    let noun = if sessions == 1 { "session" } else { "sessions" };
    Ok(DaySummary {
        text: format!("{cost} · {sessions} {noun}"),
        cost,
        sessions,
    })
}

/// The jobs a subagent's child session runs as (`subagent::Preset::job`).
fn is_subagent(job: &Job) -> bool {
    matches!(job, Job::Explore | Job::Shell | Job::Agent)
}

/// In is what the calls sent (input and cache), as the meter's `↑ Sent`.
fn row(label: String, sum: Option<Usage>, detail: bool) -> CostRow {
    let u = sum.unwrap_or(Usage {
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        estimated: false,
        cost_usd: 0.0,
        latency_ms: 0,
    });
    CostRow {
        label,
        values: vec![
            tokens(u.context_tokens()),
            tokens(u.output_tokens),
            format!(
                "{}/{}",
                tokens(u.cache_read_tokens),
                tokens(u.cache_write_tokens)
            ),
            format!("{:.2}", u.cost_usd),
        ],
        detail,
    }
}

#[cfg(test)]
mod tests {
    use cox_protocol::ids::SessionId;
    use cox_protocol::traits::UsageRow;
    use cox_protocol::types::{ModelId, ProviderId, Tier};

    use super::*;

    fn at(second: u32, turn: u32, job: Job, tier: Tier, input: u32, cost: f64) -> LedgerRow {
        LedgerRow {
            usage: UsageRow {
                session_id: SessionId::new(),
                turn,
                job,
                tier,
                provider: ProviderId::Anthropic,
                model: ModelId("m".into()),
                effort: None,
                usage: Usage {
                    input_tokens: input,
                    output_tokens: 100,
                    cache_read_tokens: 1000,
                    cache_write_tokens: 0,
                    estimated: false,
                    cost_usd: cost,
                    latency_ms: 0,
                },
            },
            created_at: format!("2026-09-28T10:00:{second:02}.000Z"),
        }
    }

    fn labels(costs: &TurnCosts) -> Vec<(&str, bool)> {
        costs
            .rows
            .iter()
            .map(|r| (r.label.as_str(), r.detail))
            .collect()
    }

    #[test]
    fn a_turn_starts_where_the_call_ordinal_restarts_and_side_calls_join_it() {
        let own = [
            at(1, 1, Job::Main, Tier::Code, 500, 0.10),
            at(2, 2, Job::Main, Tier::Code, 500, 0.10),
            at(3, 0, Job::Compact, Tier::Cheap, 500, 0.01),
            at(4, 1, Job::Main, Tier::Code, 500, 0.10),
        ];
        let costs = build(&own, &[], &BudgetConfig::default(), 0.0);
        assert_eq!(labels(&costs), [("1 · code", false), ("2 · code", false)]);
        assert_eq!(costs.rows[0].values, ["4.5k", "300", "3.0k/0", "0.21"]);
        assert_eq!(costs.rows[1].values[3], "0.10");
        assert_eq!(costs.total.label, "Session");
        assert_eq!(costs.total.values, ["6.0k", "400", "4.0k/0", "0.31"]);
    }

    #[test]
    fn a_subagent_sits_under_the_turn_it_started_in_and_a_fork_is_left_out() {
        let own = [
            at(1, 1, Job::Main, Tier::Code, 500, 0.10),
            at(5, 2, Job::Main, Tier::Code, 500, 0.10),
            at(6, 1, Job::Main, Tier::Code, 500, 0.10),
        ];
        let explore = vec![
            at(2, 1, Job::Explore, Tier::Cheap, 100, 0.01),
            at(3, 2, Job::Explore, Tier::Cheap, 100, 0.01),
        ];
        let fork = vec![at(7, 1, Job::Main, Tier::Code, 900, 9.0)];
        let costs = build(&own, &[explore, fork], &BudgetConfig::default(), 0.0);
        assert_eq!(
            labels(&costs),
            [("1 · code", false), ("explore", true), ("2 · code", false)]
        );
        assert_eq!(costs.rows[1].values, ["2.2k", "200", "2.0k/0", "0.02"]);
        assert_eq!(costs.total.values[3], "0.32");
        // The subagent's rows are the session's spend, the fork's are not.
        assert_eq!(costs.budget[0].text, "$0.32 of $5.00");
    }

    #[test]
    fn a_side_call_before_the_first_turn_gets_its_own_row() {
        let own = [
            at(1, 0, Job::Summarize, Tier::Cheap, 10, 0.0),
            at(2, 1, Job::Main, Tier::Code, 10, 0.0),
        ];
        let costs = build(&own, &[], &BudgetConfig::default(), 0.0);
        assert_eq!(
            labels(&costs),
            [("Before turn 1", false), ("1 · code", false)]
        );
    }

    #[test]
    fn today_and_this_week_start_at_local_midnight_and_monday() {
        let zone = chrono::FixedOffset::east_opt(3 * 3600).expect("offset");
        // Wednesday 01:30 at UTC+3 is still Tuesday in UTC.
        let now = zone
            .with_ymd_and_hms(2026, 9, 30, 1, 30, 0)
            .single()
            .expect("time");
        assert_eq!(
            periods(&now),
            (
                "2026-09-29T21:00:00.000Z".to_string(),
                "2026-09-27T21:00:00.000Z".to_string()
            )
        );
    }

    /// A scratch store with `sessions` sessions, each with one `usage` row
    /// per cost in `costs`.
    fn ledger(sessions: usize, costs: &[f64]) -> (tempfile::TempDir, Store) {
        use cox_protocol::Store as _;
        use cox_protocol::traits::SessionRow;

        let home = tempfile::tempdir().expect("tempdir");
        let store = Store::open(home.path()).expect("store");
        for _ in 0..sessions {
            let id = SessionId::new();
            store
                .session_create(&SessionRow {
                    id,
                    created_at: String::new(),
                    cwd: "/tmp/work".into(),
                    project_slug: "work".into(),
                    title: None,
                    parent_id: None,
                    rollout_path: std::path::PathBuf::new(),
                })
                .expect("session");
            for &cost in costs {
                let mut row = at(0, 1, Job::Main, Tier::Code, 10, cost).usage;
                row.session_id = id;
                store.usage_insert(&row).expect("usage");
            }
        }
        (home, store)
    }

    fn tomorrow() -> DateTime<chrono::Local> {
        chrono::Local::now()
            .checked_add_days(Days::new(1))
            .expect("tomorrow")
    }

    #[test]
    fn today_sums_only_todays_usage_rows() {
        let (_home, store) = ledger(1, &[0.5, 0.25]);
        let today = day_summary(&store, &chrono::Local::now()).expect("today");
        assert_eq!(today.cost, "$0.75");
        // Seen from tomorrow, the same rows are yesterday's.
        let later = day_summary(&store, &tomorrow()).expect("tomorrow");
        assert_eq!(later.cost, "$0.00");
    }

    #[test]
    fn today_counts_sessions_active_today() {
        let (_home, store) = ledger(2, &[0.1]);
        let today = day_summary(&store, &chrono::Local::now()).expect("today");
        assert_eq!(today.sessions, 2);
        assert_eq!(today.text, "$0.20 · 2 sessions");
        let later = day_summary(&store, &tomorrow()).expect("tomorrow");
        assert_eq!(
            (later.sessions, later.text.as_str()),
            (0, "$0.00 · 0 sessions")
        );
    }

    #[test]
    fn the_footnote_names_the_project_and_both_sums() {
        assert_eq!(
            footnote("cox", 3.18, 21.4),
            "Project cox today: $3.18 · this week: $21.40. \
             Every number is a row in the cost ledger."
        );
    }

    #[test]
    fn an_empty_ledger_has_no_rows_and_a_zero_total() {
        let costs = build(&[], &[], &BudgetConfig::default(), 0.0);
        assert!(costs.rows.is_empty());
        assert_eq!(costs.columns, ["In", "Out", "Cache r/w", "$"]);
        assert_eq!(costs.total.values, ["0", "0", "0/0", "0.00"]);
    }

    #[test]
    fn budget_rows_show_the_fraction_of_each_cap() {
        let rows = budget_rows(&BudgetConfig::default(), 0.5, 25.0);
        let seen: Vec<_> = rows
            .iter()
            .map(|r| (r.label.as_str(), r.text.as_str(), r.fraction))
            .collect();
        assert_eq!(
            seen,
            [
                ("Session", "$0.50 of $5.00", Some(0.1)),
                ("This month", "$25.00 of $100.00", Some(0.25)),
            ]
        );
    }

    #[test]
    fn a_spend_over_its_cap_fills_the_gauge_and_no_more() {
        let rows = budget_rows(&BudgetConfig::default(), 7.5, 0.0);
        assert_eq!(rows[0].text, "$7.50 of $5.00");
        assert_eq!(rows[0].fraction, Some(1.0));
    }

    #[test]
    fn a_cap_that_is_not_positive_leaves_the_spend_alone() {
        let budget = BudgetConfig {
            session_usd: 0.0,
            monthly_usd: f64::NAN,
            ..BudgetConfig::default()
        };
        let rows = budget_rows(&budget, 0.42, 3.0);
        assert_eq!((rows[0].text.as_str(), rows[0].fraction), ("$0.42", None));
        assert_eq!((rows[1].text.as_str(), rows[1].fraction), ("$3.00", None));
    }

    #[test]
    fn the_month_starts_at_local_midnight_of_the_first() {
        let zone = chrono::FixedOffset::east_opt(3 * 3600).expect("offset");
        // The 1st at 01:30 at UTC+3 is still the last day of August in UTC.
        let now = zone
            .with_ymd_and_hms(2026, 9, 1, 1, 30, 0)
            .single()
            .expect("time");
        assert_eq!(month_start(&now), "2026-08-31T21:00:00.000Z");
    }

    #[test]
    fn month_spend_counts_only_this_month() {
        let (_home, store) = ledger(1, &[0.5, 0.25]);
        let now = chrono::Local::now();
        assert_eq!(month_spend(&store, &now).expect("month"), 0.75);
        // Seen from a later month, the same rows are old.
        let later = now.checked_add_days(Days::new(40)).expect("later");
        assert_eq!(month_spend(&store, &later).expect("later"), 0.0);
    }
}
