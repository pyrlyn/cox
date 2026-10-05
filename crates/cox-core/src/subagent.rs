// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Subagents (plan.md T3.9): the `agent` tool runs one task in a child
//! `Session` on its own tier with a tool allowlist, a budget slice and a
//! result cap. It lives in `cox-core` rather than `cox-tools` because a
//! child session *is* the loop, not I/O, and `cox-tools` may not depend on
//! this crate; the presets are plain data here for the same reason.
//!
//! T34.1: `preset` also resolves a discovered `AgentDef` (`.cox/agents`,
//! `.claude/agents`) by name, so a custom definition dispatches exactly
//! like `explore`/`shell`. `AgentDef`/`tier_for` live in
//! `cox_protocol::agent`, not `cox-ext` (which reads the filesystem and
//! this crate may not depend on): the surface
//! (`crates/cox-session`'s `open`) runs discovery once at session build and
//! hands the result to `Session::set_agent_defs`, keeping this crate
//! I/O-free. A custom preset's usage rows are tagged `Job::Agent`; its own
//! `tier`/`model` decides the actual tier, not the job.
//!
//! T34.5: a subagent keeps answering follow-ups (SM§2, §3, §5). The parent
//! routes every message — a child never holds a sibling's handle — and
//! owns the causal hop count; delivery is always a whole new turn.
//!
//! T35.5: a granted `[[external_agents]]` entry is one more name `preset`
//! resolves (EA§3). Its child is an ordinary child session whose turns
//! run on the host's `ExternalAgent` driver instead of the model
//! (`Session::external_turn`), so follow-ups, sibling messages, parking
//! and waking all take the T34.5 path above unchanged.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use cox_permission::narrower;
use cox_protocol::errors::{CoreError, ToolError};
use cox_protocol::ids::{CallId, ItemId, SessionId, TaskId};
use cox_protocol::traits::{ExternalAgent, Relay, Tool, ToolCx, Worktree};
use cox_protocol::types::{
    Concurrency, Content, DecidedBy, Decision, Event, HookEvent, HookOutcome, Job, Level, Message,
    ModelId, PermissionMode, ProviderEvent, Request, Risk, Role, Source, Submission, SystemBlock,
    Tier, ToolCall, ToolOutput, ToolSpec, Why,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::budget;
use crate::hooks;
use crate::rollout::History;
use crate::session::Session;
use crate::tasks::{Queued, cost_detail, message_line};

/// A subagent shape: which job it reports as, which tools it may use, how
/// long it may run and how big its answer may be.
// `Job::Plugin`'s `String` payload (T33.15) means `Job`, and so `Preset`,
// can no longer be `Copy`; `resolve` below clones a `&'static Preset`
// instead of copying it.
#[derive(Debug, Clone)]
pub struct Preset {
    /// What the model passes as `preset`.
    pub name: &'static str,
    /// The job its provider calls are recorded under; picks the tier.
    pub job: Job,
    /// Tool names the child may use.
    pub tools: &'static [&'static str],
    /// Every tool must be `ReadOnly`, whatever the allowlist says.
    pub read_only: bool,
    /// Provider calls the child may make for one task.
    pub max_turns: u32,
    /// Results longer than this (≈ 4 bytes per token) are summarised.
    pub result_cap_tokens: usize,
}

/// Read-only file exploration on the cheap tier, short answer.
pub const EXPLORE: Preset = Preset {
    name: "explore",
    job: Job::Explore,
    tools: &["read", "grep", "glob", "outline", "expand"],
    read_only: true,
    max_turns: 30,
    result_cap_tokens: 1000,
};

/// Builds, tests and HTTP calls whose full output the parent does not need.
pub const SHELL: Preset = Preset {
    name: "shell",
    job: Job::Shell,
    tools: &["bash", "web_fetch"],
    read_only: false,
    max_turns: 30,
    result_cap_tokens: 2000,
};

const PRESETS: &[Preset] = &[EXPLORE, SHELL];

/// A generic, non-read-only default for a discovered `AgentDef`: the
/// same shape as [`SHELL`], since a custom preset's own `tools:` (not
/// `read_only`) is what narrows it.
const CUSTOM_MAX_TURNS: u32 = SHELL.max_turns;
const CUSTOM_RESULT_CAP_TOKENS: usize = SHELL.result_cap_tokens;

/// A dispatch target flattened from either a built-in [`Preset`] or a
/// discovered `AgentDef`, so `tools_for`/`call` match on it once instead
/// of on the source everywhere they need a field.
struct Resolved {
    name: String,
    job: Job,
    /// `None` means every parent tool (an `AgentDef` with an empty
    /// `tools:`); built-in presets always name theirs explicitly.
    tools: Option<Vec<String>>,
    read_only: bool,
    max_turns: u32,
    result_cap_tokens: usize,
    /// The tier this dispatch would run at with no `tier` override: the
    /// job's configured tier for a built-in preset, or the def's own
    /// `model`/`tier_for`, falling back to the parent's tier when the
    /// model is `inherit` or absent (`cox_protocol::agent::tier_for`).
    natural_tier: Tier,
    /// The driver an external-agent preset runs on (T35.5); `None` for a
    /// model-driven one.
    external: Option<Arc<dyn ExternalAgent>>,
    /// The mode an `AgentDef`'s `permissionMode` asks for (T45.2); `call`
    /// runs the child in the narrower of it and the parent's live mode.
    permission: Option<PermissionMode>,
}

impl std::fmt::Debug for Resolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resolved")
            .field("name", &self.name)
            .field("external", &self.external.is_some())
            .finish_non_exhaustive()
    }
}

/// What a subagent may spend when the call does not say: a quarter of
/// what the parent has left, so four background explorers cannot drain it.
const DEFAULT_SLICE: f64 = 0.25;

/// The child's session cap in USD: never more than the parent has left.
pub fn slice(parent_cap: f64, parent_spent: f64, requested: Option<f64>) -> f64 {
    let remaining = (parent_cap - parent_spent).max(0.0);
    requested.map_or(remaining * DEFAULT_SLICE, |r| r.max(0.0).min(remaining))
}

/// `agent`: delegates a task to a child session and returns its answer.
pub struct AgentTool {
    parent: Session,
    /// Children spawned so far; numbers their names (`explore-2`, T27.2).
    spawned: AtomicU32,
}

/// Discovered names not already shadowed by a built-in preset, and not
/// `disabled: true` (T34.10) — used in the tool description, the "unknown
/// preset" error and `Session::agent_names`, so none of them disagrees
/// about what is dispatchable. A disabled def still discovers (`cox ext
/// list` shows it, marked); it just never appears here.
fn custom_names(session: &Session) -> Vec<String> {
    session
        .agent_defs()
        .iter()
        .filter(|d| !d.disabled)
        .map(|d| d.name.clone())
        .filter(|n| !PRESETS.iter().any(|p| p.name == n))
        .collect()
}

/// Granted external-agent names (T35.5) not shadowed by a built-in or
/// discovered preset, which resolve first.
fn external_names(session: &Session) -> Vec<String> {
    let custom = custom_names(session);
    session
        .external_agents()
        .iter()
        .map(|a| a.name().to_string())
        .filter(|n| !PRESETS.iter().any(|p| p.name == n) && !custom.contains(n))
        .collect()
}

impl Session {
    /// Every name `agent(preset: …)` resolves, in resolution order:
    /// built-in presets, enabled definitions, granted external agents. A
    /// surface offers these for `@name task` (T45.6).
    pub fn agent_names(&self) -> Vec<String> {
        PRESETS
            .iter()
            .map(|p| p.name.to_string())
            .chain(custom_names(self))
            .chain(external_names(self))
            .collect()
    }
}

impl AgentTool {
    pub(crate) fn new(parent: Session) -> Self {
        Self {
            parent,
            spawned: AtomicU32::new(0),
        }
    }

    fn custom_names(&self) -> Vec<String> {
        custom_names(&self.parent)
    }

    fn external_names(&self) -> Vec<String> {
        external_names(&self.parent)
    }

    /// Built-in `PRESETS` first (unchanged behaviour for `explore`/`shell`,
    /// even if a same-named file is discovered), then a discovered
    /// `AgentDef` by exact name; a miss lists both.
    fn resolve(&self, input: &Value) -> Result<Resolved, ToolError> {
        let name = input
            .get("preset")
            .and_then(Value::as_str)
            .unwrap_or(EXPLORE.name);
        if let Some(p) = PRESETS.iter().find(|&p| p.name == name).cloned() {
            return Ok(Resolved {
                name: p.name.to_string(),
                job: p.job.clone(),
                tools: Some(p.tools.iter().map(|s| s.to_string()).collect()),
                read_only: p.read_only,
                max_turns: p.max_turns,
                result_cap_tokens: p.result_cap_tokens,
                natural_tier: self.parent.config.jobs.tier_for(p.job),
                external: None,
                permission: None,
            });
        }
        if let Some(def) = self
            .parent
            .agent_defs()
            .iter()
            .find(|d| d.name == name && !d.disabled)
        {
            return Ok(Resolved {
                name: def.name.clone(),
                job: Job::Agent,
                tools: (!def.tools.is_empty()).then(|| def.tools.clone()),
                read_only: false,
                max_turns: CUSTOM_MAX_TURNS,
                result_cap_tokens: CUSTOM_RESULT_CAP_TOKENS,
                natural_tier: cox_protocol::agent::tier_for(def.model.as_deref())
                    .unwrap_or(self.parent.tier),
                external: None,
                permission: def.permission_mode,
            });
        }
        if let Some(agent) = self
            .parent
            .external_agents()
            .iter()
            .find(|a| a.name() == name)
        {
            // No cox tools: the external agent runs its own inside its
            // own sandboxed process (EA§2).
            return Ok(Resolved {
                name: name.to_string(),
                job: Job::Agent,
                tools: Some(Vec::new()),
                read_only: false,
                max_turns: CUSTOM_MAX_TURNS,
                result_cap_tokens: CUSTOM_RESULT_CAP_TOKENS,
                natural_tier: self.parent.tier,
                external: Some(agent.clone()),
                permission: None,
            });
        }
        let names = self.parent.agent_names();
        Err(ToolError::Denied {
            why: format!(
                "unknown agent preset {name:?}; available presets: {}",
                names.join(", ")
            ),
        })
    }

    /// The `tier` a run at `resolved.natural_tier` may be asked to switch
    /// to: only down (D5 "never up"), and only to a name the router knows.
    fn resolve_tier(&self, resolved: &Resolved, input: &Value) -> Result<Tier, ToolError> {
        let Some(raw) = input.get("tier").and_then(Value::as_str) else {
            return Ok(resolved.natural_tier);
        };
        let requested = parse_tier(raw).ok_or_else(|| ToolError::Denied {
            why: format!("unknown tier {raw:?}; use cheap, code or think"),
        })?;
        let clamped = if tier_rank(requested) <= tier_rank(resolved.natural_tier) {
            requested
        } else {
            resolved.natural_tier
        };
        Ok(clamped)
    }

    /// The parent's tools this call may hand to the child: the resolved
    /// allowlist (or the call's `tools`, narrowed to it for a read-only
    /// preset), never `agent` itself.
    fn tools_for(&self, resolved: &Resolved, input: &Value) -> Vec<Arc<dyn Tool>> {
        if resolved.external.is_some() {
            return Vec::new();
        }
        let wanted: Option<Vec<String>> = input
            .get("tools")
            .and_then(Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .or_else(|| resolved.tools.clone());
        self.parent
            .tools
            .iter()
            .filter(|t| {
                let spec = t.spec();
                spec.name != "agent"
                    && wanted.as_ref().is_none_or(|w| w.contains(&spec.name))
                    && (!resolved.read_only || spec.risk == Risk::ReadOnly)
            })
            .cloned()
            .collect()
    }
}

/// `cheap` < `code` < `think`, for the "never up" clamp (D5); `Tier` has
/// no `Ord` of its own because nothing else needs to compare tiers.
fn tier_rank(t: Tier) -> u8 {
    match t {
        Tier::Cheap => 0,
        Tier::Code => 1,
        Tier::Think => 2,
    }
}

fn parse_tier(s: &str) -> Option<Tier> {
    match s {
        "cheap" => Some(Tier::Cheap),
        "code" => Some(Tier::Code),
        "think" => Some(Tier::Think),
        _ => None,
    }
}

#[async_trait]
impl Tool for AgentTool {
    fn spec(&self) -> ToolSpec {
        let mut description = "Delegate a self-contained task to a subagent that runs on the \
                cheap tier with its own tool set and budget, and returns only its answer. \
                Presets: `explore` (read-only file tools, answer ≤ 1k tokens) for \"find \
                where X is handled and report file:line\", `shell` (bash, web_fetch) for \
                builds, test runs and HTTP calls whose full output you do not need. Pass \
                `task` with everything the subagent needs to know; it does not see this \
                conversation. Optional: `tools` to narrow the tool list, `tier` (`cheap`, \
                `code` or `think`) to run the task on a cheaper tier than its preset's \
                default — never a more expensive one, `budget_usd`, \
                `isolation: \"worktree\"` to run the task in its own git worktree and \
                branch (named after the task id) so its edits never touch this checkout; \
                the answer then ends with the worktree path and branch."
            .to_string();
        let custom = self.custom_names();
        if !custom.is_empty() {
            description.push_str(&format!(
                " Custom presets from `.cox/agents`/`.claude/agents`: {}.",
                custom.join(", ")
            ));
        }
        let external = self.external_names();
        if !external.is_empty() {
            description.push_str(&format!(
                " External agents from plugins, which run their own tools: {}.",
                external.join(", ")
            ));
        }
        ToolSpec {
            name: "agent".to_string(),
            description,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string"},
                    "preset": {"type": "string"},
                    "tier": {"type": "string", "enum": ["cheap", "code", "think"]},
                    "tools": {"type": "array", "items": {"type": "string"}},
                    "budget_usd": {"type": "number", "minimum": 0},
                    "background": {"type": "boolean"},
                    "isolation": {"type": "string", "enum": ["none", "worktree"]}
                },
                "required": ["task"]
            }),
            deferred: true,
            risk: Risk::ReadOnly,
            concurrency: Concurrency::Parallel,
        }
    }

    fn subject(&self, input: &Value) -> String {
        self.resolve(input).map_or_else(|_| "?".into(), |r| r.name)
    }

    /// The riskiest tool the child may use (plan.md §1.11: "inherits max
    /// of its tools").
    fn risk(&self, input: &Value) -> Risk {
        let Ok(resolved) = self.resolve(input) else {
            return Risk::Exec;
        };
        // `git worktree add` changes the repository whatever the child's
        // tools are, so it must ask even for a read-only preset (T44.1).
        if input.get("isolation").and_then(Value::as_str) == Some("worktree") {
            return Risk::Destructive;
        }
        // Its own tools are invisible to cox, so it counts as running
        // anything (the same class `StreamJsonMapper` gives its calls).
        if resolved.external.is_some() {
            return Risk::Exec;
        }
        self.tools_for(&resolved, input)
            .iter()
            .map(|t| t.spec().risk)
            .max_by_key(|r| rank(*r))
            .unwrap_or(Risk::ReadOnly)
    }

    async fn call(&self, input: Value, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let task_text = input
            .get("task")
            .and_then(Value::as_str)
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| ToolError::Denied {
                why: "missing or empty \"task\"".into(),
            })?
            .to_string();
        // T34.2 review: a burst of parallel `agent` calls in one turn (the
        // core dispatches `Concurrency::Parallel` tools concurrently) must
        // not all pass a count-then-register check — `try_reserve_agent_slot`
        // checks the cap and reserves a slot in one atomic step. The guard
        // frees it on drop: kept as a local for the foreground path (freed
        // when `call()` returns, unless a message raced the parking and
        // moves it into that continuation instead — T34.6 point 4, below)
        // and moved into the background closure below (freed when that
        // child actually finishes).
        let cap = self.parent.config.core.max_concurrent_subagents;
        let agent_slot =
            self.parent
                .try_reserve_agent_slot(cap)
                .map_err(|running| ToolError::Denied {
                    why: format!(
                        "subagent concurrency cap reached: {running} of {cap} \
                         `agent` tasks already running"
                    ),
                })?;
        let preset = self.resolve(&input)?;
        let tools = self.tools_for(&preset, &input);
        let tier = self.resolve_tier(&preset, &input)?;
        let mut config = self.parent.config.clone();
        // T45.1: the parent's live mode, not the configured one, so a child
        // spawned after Shift+Tab to plan is never wider than its parent.
        // Session grants stay the parent's own. T45.2: a def's
        // `permissionMode` may narrow that, never widen it; the child's own
        // `cox_permission::Engine` then decides every call as usual.
        let live = self.parent.permission_mode().await;
        config.permissions.mode = match preset.permission {
            Some(asked) if narrower(live, asked) != asked => {
                let name = |mode: PermissionMode| format!("{mode:?}").to_lowercase();
                let text = format!(
                    "`{}` asked for `{}`, runs as `{}`",
                    preset.name,
                    name(asked),
                    name(live)
                );
                self.parent
                    .notice(Level::Info, text)
                    .await
                    .map_err(core_error)?;
                live
            }
            Some(asked) => asked,
            None => live,
        };
        config.budget.session_usd = slice(
            config.budget.session_usd,
            self.parent.spent().await,
            input.get("budget_usd").and_then(Value::as_f64),
        );
        config.core.max_turns = preset.max_turns;
        let task = TaskId::new();
        // T27.3: the child works in `_worktrees/<repo>-<task>` on branch
        // `<task>`, with the main checkout as a second root so it can still
        // read what the parent sees; the worktree outlives the task for
        // the user to merge.
        let worktree = match input.get("isolation").and_then(Value::as_str) {
            Some("worktree") => {
                let worktrees = self.parent.worktrees().ok_or_else(|| ToolError::Denied {
                    why: "worktree isolation is not available on this surface".into(),
                })?;
                let owner = format!("cox / {}", self.parent.id);
                let wt = worktrees
                    .add(&self.parent.cwd, &task.to_string(), &owner)
                    .await
                    .map_err(|e| ToolError::Denied {
                        why: format!("worktree: {e}"),
                    })?;
                config.core.workspace_roots = vec![wt.path.clone(), wt.main.clone()];
                Some(wt)
            }
            Some("none") | None => None,
            Some(other) => {
                return Err(ToolError::Denied {
                    why: format!("unknown isolation {other:?}; use none or worktree"),
                });
            }
        };
        let spec = Spec {
            config,
            tools,
            job: preset.job,
            tier,
            worktree,
            name: format!(
                "{}-{}",
                preset.name,
                self.spawned.fetch_add(1, Ordering::Relaxed) + 1
            ),
            preset_name: preset.name.clone(),
            result_cap_tokens: preset.result_cap_tokens,
            label: format!("{}: {}", preset.name, first_line(&task_text)),
            external: preset.external.clone(),
        };
        let child = spawn(&self.parent, task, &spec, None).map_err(core_error)?;
        let Some(events) = child.events() else {
            return Err(ToolError::Io);
        };
        let label = spec.label.clone();
        // SubagentStart gates both paths; a Block means the task never existed.
        if let HookOutcome::Block { reason } = hooks::fire(
            &self.parent,
            HookEvent::SubagentStart,
            json!({"task": task.to_string(), "label": label, "preset": preset.name}),
        )
        .await
        {
            return Err(ToolError::Denied {
                why: format!("subagent blocked by hook: {reason}"),
            });
        }
        self.parent
            .emit(Event::TaskCreated {
                task,
                label: label.clone(),
                tier,
            })
            .await
            .map_err(core_error)?;
        self.parent
            .register_task(task, label.clone(), tier, crate::tasks::TaskKind::Agent)
            .await;
        self.parent.track_child(task).await;
        // T34.6: a `send_message` call from the parent addresses this child
        // by `spec.name` (`explore-2`), never its `TaskId` directly.
        self.parent.name_task(spec.name.clone(), task).await;
        let mut io = RunIo {
            task,
            spec,
            events,
            cancel: cx.cancel.clone(),
            progress: cx.output.clone(),
        };
        if input
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let parent = self.parent.clone();
            tokio::spawn(async move {
                // T34.2: the slot stays reserved for this child's whole run,
                // not just until `call()` returns its "started" pointer.
                let _agent_slot = agent_slot;
                drive(parent, task, child, task_text, io).await;
            });
            return Ok(ToolOutput {
                text: format!(
                    "background task {task} started: {label}\n\
                     its result will arrive as a notice, not in this turn"
                ),
                is_error: false,
                diff: None,
                structured: Some(json!({
                    "task": task,
                    "preset": preset.name,
                    "background": true,
                })),
            });
        }

        let session = child.id();
        let outcome = run_task(&self.parent, child, task_text, &mut io).await;
        let cost_usd = outcome.as_ref().map(|o| o.cost_usd).unwrap_or(0.0);
        let completed = self
            .parent
            .emit(Event::TaskCompleted {
                task,
                result_item: ItemId::new(),
                cost_usd,
                exit_code: None,
                archive: None,
            })
            .await;
        let _ = hooks::fire(
            &self.parent,
            HookEvent::SubagentStop,
            json!({"task": task.to_string(), "label": label}),
        )
        .await;
        io.spec.charge(cost_usd);
        let parked = self.parent.park_child(
            task,
            Dormant {
                session,
                spec: io.spec,
            },
        );
        if let Some((dormant, next)) = parked.await {
            // T34.6 point 4: a message that raced the parking continues
            // the very run this `call()` is still holding `agent_slot`
            // for — not a brand-new wake — so the continuation takes that
            // same guard with it instead of reserving (or being denied) a
            // second one. Only a genuinely new wake from `Finished`
            // (`Session::deliver`, `tasks.rs`) reserves its own.
            //
            // T34.9: the registry entry stays in place (`register_task`,
            // above) rather than being dropped and re-added — `complete_task`
            // only runs once this chain truly ends (the `else` below, or
            // `lost`) — so `wait_idle` never wakes into the gap between
            // this parking and `wake`'s spawned task actually resuming it.
            let parent = self.parent.clone();
            tokio::spawn(async move {
                let _slot = agent_slot;
                wake(parent, task, dormant, next).await;
            });
        } else {
            // A failed task still finished: drop the registry entry and
            // close the Created/Completed pair so `/tasks` never shows a
            // ghost, and so `wait_idle` sees this task as done.
            self.parent.complete_task(task).await;
        }
        completed.map_err(core_error)?;
        let outcome = outcome?;
        Ok(ToolOutput {
            text: outcome.answer,
            is_error: false,
            diff: None,
            structured: Some(json!({
                "task": task,
                "preset": preset.name,
                "turns": outcome.turns,
                "cost_usd": outcome.cost_usd,
                "summarised": outcome.summarised,
            })),
        })
    }
}

/// T27.2: the child's own event stream has no surface, so its prompt is
/// raised on the parent's, labelled with the agent, and the parent's
/// `Submission::Approve` for that call is handed back to the child. If the
/// child is cancelled first it answers itself `Deny`, and the relayed
/// `ApprovalDecided` closes the prompt.
async fn relay_approval(parent: &Session, child: &Session, call: ToolCall, why: Why, io: &RunIo) {
    let id = call.id;
    let decision = parent.relay_decision(id).await;
    let source = Source {
        session: child.id(),
        agent: Some(io.spec.name.clone()),
        preset: Some(io.spec.preset_name.clone()),
    };
    let asked = parent.emit(Event::ApprovalRequired {
        call,
        why,
        source: Some(source),
    });
    if asked.await.is_err() {
        return;
    }
    let child = child.clone();
    tokio::spawn(async move {
        let decision = decision.await.unwrap_or(Decision::Deny {
            reason: "session closed".into(),
        });
        let _ = child
            .submit(Submission::Approve {
                call_id: id,
                decision,
            })
            .await;
    });
}

/// DT G4: a child's `QuestionAsked` has no surface either, so the parent
/// raises it on its own stream (the tool already labelled `source`) and
/// hands the parent's `Submission::Answer` back to the child.
fn relay_question(
    parent: &Session,
    child: &Session,
    call_id: CallId,
    question: String,
    options: Vec<String>,
    source: Option<Source>,
) {
    let (parent, child) = (parent.clone(), child.clone());
    tokio::spawn(async move {
        let text = parent
            .raise_question(call_id, &question, &options, source)
            .await
            .ok()
            .flatten();
        let _ = child.submit(Submission::Answer { call_id, text }).await;
    });
}

/// What one child run produced, foreground or background.
struct TaskOutcome {
    answer: String,
    cost_usd: f64,
    turns: u32,
    summarised: bool,
}

/// Everything a child needs to run again after it finished (SM§2): the
/// same job, tier, tools, parent and what is left of its budget slice, and
/// how its runs are labelled and capped.
#[derive(Clone)]
pub(crate) struct Spec {
    config: cox_protocol::Config,
    tools: Vec<Arc<dyn Tool>>,
    job: Job,
    tier: Tier,
    /// The child's worktree, named in the answer so the parent can merge it.
    worktree: Option<Worktree>,
    /// `<preset>-<n>`: how its approvals are labelled on the parent's surface.
    name: String,
    preset_name: String,
    result_cap_tokens: usize,
    label: String,
    /// Carried so a woken child resumes on the same driver (T35.5).
    external: Option<Arc<dyn ExternalAgent>>,
}

impl Spec {
    /// A resumed run gets what the finished ones left of the slice.
    fn charge(&mut self, cost_usd: f64) {
        let left = self.config.budget.session_usd - cost_usd;
        self.config.budget.session_usd = left.max(0.0);
    }
}

/// A finished child: its stored session, resumed from the rollout.
pub(crate) struct Dormant {
    session: SessionId,
    spec: Spec,
}

impl Dormant {
    /// What `Session::deliver`'s `Wake` case needs to re-register the task
    /// (`tasks.rs`) before spawning `wake`, below, so a woken dormant child
    /// counts as running immediately rather than once its restart actually
    /// gets scheduled (T34.9).
    pub(crate) fn label_and_tier(&self) -> (String, Tier) {
        (self.spec.label.clone(), self.spec.tier)
    }
}

/// The child session for `spec`, fresh or restored from `resume`. `task` is
/// the parent's id for this child, stamped onto it (`self_task`, T34.6) so
/// its own `send_message` calls know which task id means "the parent"
/// (SM§4): a child compares `to` against this, never its own `SessionId`.
fn spawn(
    parent: &Session,
    task: TaskId,
    spec: &Spec,
    resume: Option<(SessionId, History)>,
) -> Result<Session, CoreError> {
    let cwd = spec.worktree.as_ref().map(|wt| wt.path.clone());
    let (config, tools) = (spec.config.clone(), spec.tools.clone());
    let child = parent.spawn_child(
        config,
        tools,
        spec.job.clone(),
        spec.tier,
        cwd,
        resume,
        spec.name.clone(),
        spec.preset_name.clone(),
    )?;
    if let Some(wt) = &spec.worktree {
        child.set_writable_roots(vec![wt.path.clone()]);
    }
    child.set_self_task(task);
    if let Some(agent) = &spec.external {
        child.set_external(agent.clone());
    }
    Ok(child)
}

/// Runs a background child until nothing is left for it to answer: each
/// run reports like any background task, then a message that arrived
/// meanwhile resumes it rather than waiting for another `wake` (SM§2).
async fn drive(parent: Session, task: TaskId, mut child: Session, text: String, mut io: RunIo) {
    let mut text = text;
    loop {
        let session = child.id();
        let (answer, cost_usd) = match run_task(&parent, child, text, &mut io).await {
            Ok(o) => (o.answer, o.cost_usd),
            Err(e) => (format!("task failed: {e}"), 0.0),
        };
        let label = io.spec.label.clone();
        let _ = parent
            .emit(Event::TaskCompleted {
                task,
                result_item: ItemId::new(),
                cost_usd,
                exit_code: None,
                archive: None,
            })
            .await;
        let _ = parent
            .publish_task_result(task, &label, &answer, &cost_detail(cost_usd))
            .await;
        let stop = json!({"task": task.to_string(), "label": label});
        let _ = hooks::fire(&parent, HookEvent::SubagentStop, stop).await;
        io.spec.charge(cost_usd);
        let spec = io.spec.clone();
        // T34.9: the registry entry (`register_task`, from the initial
        // dispatch or a previous `restart`) stays in place across this
        // call — `complete_task` only runs once a cycle turns up nothing
        // queued (below) or the child cannot be restarted (`lost`) — so
        // `wait_idle` never wakes into the gap between one cycle ending
        // and the next, raced in by a message, actually starting.
        let Some((dormant, next)) = parent.park_child(task, Dormant { session, spec }).await else {
            parent.complete_task(task).await;
            return;
        };
        text = message_line(next.from, &next.text);
        child = match restart(&parent, task, &dormant, &mut io).await {
            Ok(child) => child,
            Err(e) => return lost(&parent, task, e).await,
        };
    }
}

/// A message woke a finished child. Boxed so `Session::deliver`, which
/// spawns it, does not need its own future's `Send` proof in a cycle.
pub(crate) fn wake(
    parent: Session,
    task: TaskId,
    dormant: Box<Dormant>,
    next: Queued,
) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(async move {
        let (progress, _) = mpsc::channel(1);
        let (_, events) = mpsc::channel(1);
        let cancel = parent.cancel_token();
        let spec = dormant.spec.clone();
        let mut io = RunIo {
            task,
            spec,
            events,
            cancel,
            progress,
        };
        match restart(&parent, task, &dormant, &mut io).await {
            Ok(child) => drive(parent, task, child, message_line(next.from, &next.text), io).await,
            Err(e) => lost(&parent, task, e).await,
        }
    })
}

/// Restores a finished child from its rollout with its own job, tier,
/// parent and budget slice, and announces it as running again.
async fn restart(
    parent: &Session,
    task: TaskId,
    dormant: &Dormant,
    io: &mut RunIo,
) -> Result<Session, CoreError> {
    let events = parent
        .store
        .rollout_read(&dormant.session)
        .map_err(|error| CoreError::Store { error })?;
    let mut history = History::from_events(&events);
    // T50.2: the parent may have narrowed since the child ran (Shift+Tab to
    // Plan); a woken child must not come back wider than the parent is now.
    let live = parent.permission_mode().await;
    let mode = history
        .permission_mode
        .map_or(live, |own| narrower(live, own));
    history.permission_mode = Some(mode);
    let child = spawn(
        parent,
        task,
        &dormant.spec,
        Some((dormant.session, history)),
    )?;
    io.events = child.events().unwrap_or_else(|| mpsc::channel(1).1);
    io.cancel = parent.cancel_token();
    let (label, tier) = (dormant.spec.label.clone(), dormant.spec.tier);
    parent
        .emit(Event::TaskCreated {
            task,
            label: label.clone(),
            tier,
        })
        .await?;
    parent
        .register_task(task, label, tier, crate::tasks::TaskKind::Agent)
        .await;
    Ok(child)
}

/// A child that could not be restored stops being addressable, loudly.
/// Also closes its registry entry (T34.9): a failed `restart` never
/// reaches its own `register_task` call, so whichever caller still had
/// this task registered (`drive`'s own mid-loop retry; `wake`'s fresh one
/// never registered it in the first place, so this is a harmless no-op
/// there) needs it cleared for `wait_idle` to see the task as done.
async fn lost(parent: &Session, task: TaskId, e: CoreError) {
    parent.complete_task(task).await;
    parent.forget_child(task).await;
    let text = format!("subagent task {task} could not resume: {e}");
    let _ = parent.notice(Level::Warn, text).await;
}

/// SM§5: hops a message may travel before the parent drops it, so an
/// A→B→A ping-pong stops. `MAX_MESSAGES_PER_TASK` is T34.6's, with the tool.
pub(crate) const MAX_HOPS: u32 = 4;

/// SM§5: a per-task received-message flood cap, checked by `Session::deliver`
/// (`tasks.rs`) before a `send_message` follow-up is queued or wakes a
/// dormant child. A named constant, not a config key: raising it is a code
/// change, not a per-session tuning knob.
pub(crate) const MAX_MESSAGES_PER_TASK: u32 = 16;

/// A child's `Event::TaskMessage`, routed by the parent only (SM§3): to its
/// own task id means "to the parent", anything else is a sibling reached
/// through the parent's own `Submission::TaskMessage`. `from` and `hop`
/// are the parent's, never what the child claimed.
pub(crate) async fn relay(
    parent: &Session,
    from: TaskId,
    to: TaskId,
    text: String,
) -> Result<(), CoreError> {
    let hop = parent.child_hop(from).await + 1;
    if hop > MAX_HOPS {
        let text = format!("message from task {from} to {to} dropped: hop limit {MAX_HOPS}");
        return parent.notice(Level::Warn, text).await;
    }
    if to == from {
        return parent.message_parent(from, hop, text).await;
    }
    let from = Some(from);
    let sub = Submission::TaskMessage {
        task: to,
        from,
        hop,
        text,
    };
    parent.submit(sub).await
}

/// `send_message`'s only way into a session (T34.6, SM§4): `cox-tools`
/// holds `Arc<dyn Relay>`, never a `Session`, so the routing lives here
/// instead of a new channel. A child (`self_task` set by `spawn`) resolves
/// `"parent"` to its own task id (matching `relay`'s `to == from` check)
/// or a sibling by its registry name or literal `TaskId`, through the same
/// `resolve_name_or_id` the parent uses (`tasks.rs`) — `spawn_child` hands
/// every child the parent's name→`TaskId` registry read-only (T34.9), so a
/// scripted scenario can address a sibling by the deterministic name it was
/// given rather than a `TaskId` it has no way to learn. The parent's own
/// call is hop 0, direct, no relay hop-count.
#[async_trait]
impl Relay for Session {
    async fn ask(
        &self,
        call_id: CallId,
        question: &str,
        options: &[String],
        source: Option<Source>,
    ) -> Result<Option<String>, ToolError> {
        self.raise_question(call_id, question, options, source)
            .await
    }

    async fn send_message(&self, to: &str, text: &str) -> Result<(), ToolError> {
        match self.self_task() {
            Some(me) => {
                let target = if to == "parent" {
                    me
                } else {
                    self.resolve_name_or_id(to)
                        .await
                        .ok_or_else(|| ToolError::Denied {
                            why: format!("unknown addressee {to:?}: no such subagent task"),
                        })?
                };
                self.emit(Event::TaskMessage {
                    task: target,
                    from: None,
                    hop: 0,
                    text: text.to_string(),
                })
                .await
                .map_err(core_error)
            }
            None => {
                let Some(target) = self.resolve_addressee(to).await else {
                    return Err(ToolError::Denied {
                        why: format!("unknown addressee {to:?}: no such subagent task"),
                    });
                };
                self.deliver(target, None, 0, text.to_string())
                    .await
                    .map_err(core_error)
            }
        }
    }
}

/// How one child run is driven and observed.
struct RunIo {
    /// The child's own task id: its queue and hop in the parent's registry.
    task: TaskId,
    spec: Spec,
    events: mpsc::Receiver<Event>,
    cancel: CancellationToken,
    progress: mpsc::Sender<String>,
}

/// The child's next turn, on its own task so the relay loop keeps running.
fn submit_turn(child: &Session, text: String) -> JoinHandle<Result<(), CoreError>> {
    let runner = child.clone();
    tokio::spawn(async move {
        runner
            .submit(Submission::UserTurn {
                text,
                attachments: vec![],
                confirm_think: false,
            })
            .await
    })
}

/// Drives the child's turns and distills its answer (shared by the
/// foreground call and the background task): accumulates cost, streams
/// progress lines, charges the parent, caps the answer. A message queued
/// during a turn becomes the next turn once `TurnDone` closes this one.
async fn run_task(
    parent: &Session,
    child: Session,
    task_text: String,
    io: &mut RunIo,
) -> Result<TaskOutcome, ToolError> {
    let mut turn = submit_turn(&child, task_text);
    let mut cost_usd = 0.0;
    let mut turns = 0u32;
    let mut interrupted = false;
    // `TurnDone` is the child's last event (turn.rs: nothing follows it).
    let outcome = loop {
        tokio::select! {
            _ = io.cancel.cancelled(), if !interrupted => {
                interrupted = true;
                child.interrupt();
            }
            ev = io.events.recv() => match ev {
                Some(Event::Usage { usage, .. }) => {
                    cost_usd += usage.cost_usd;
                    turns += 1;
                }
                Some(Event::ToolCallRequested { call }) => {
                    let _ = io
                        .progress
                        .send(format!("[{}] {}\n", io.spec.preset_name, call.name))
                        .await;
                }
                Some(Event::TurnDone { .. }) => {
                    let Some(next) = parent.next_queued(io.task).await else {
                        break Ok(());
                    };
                    if let Ok(Err(e)) = (&mut turn).await {
                        break Err(core_error(e));
                    }
                    turn = submit_turn(&child, message_line(next.from, &next.text));
                }
                Some(Event::ApprovalRequired { call, why, .. }) => {
                    relay_approval(parent, &child, call, why, io).await;
                }
                Some(Event::QuestionAsked {
                    call_id,
                    question,
                    options,
                    source,
                }) => relay_question(parent, &child, call_id, question, options, source),
                // Closes the prompt the relay opened on the parent's surface;
                // a rule's verdict never opened one.
                Some(ev @ Event::ApprovalDecided { by: DecidedBy::User, .. }) => {
                    let _ = parent.emit(ev).await;
                }
                Some(Event::TaskMessage { task: to, text, .. }) => {
                    let _ = relay(parent, io.task, to, text).await;
                }
                Some(_) => {}
                None => break Err(ToolError::Io),
            },
        }
    };
    if let Ok(Err(e)) = turn.await {
        return Err(core_error(e));
    }
    outcome?;
    if budget::counts(io.spec.tier, parent.config.budget.cheap_counts) {
        parent.add_spend(cost_usd).await;
    }

    let mut result = child
        .history()
        .await
        .into_iter()
        .rev()
        .find(|m| m.role == Role::Assistant)
        .and_then(|m| {
            m.content.into_iter().find_map(|c| match c {
                Content::Text { text } => Some(text),
                _ => None,
            })
        })
        .unwrap_or_else(|| "(the subagent produced no answer)".to_string());
    let mut summarised = false;
    let cap = io.spec.result_cap_tokens;
    if result.len() / 4 > cap {
        if let Some(short) = summarize(parent, &result, cap).await {
            result = short;
            summarised = true;
        } else {
            result.truncate(cap * 4);
            result.push_str("\n[cut at the result cap]");
        }
    }
    // After the cap, so the trailer the parent merges from is never cut.
    if let Some(wt) = &io.spec.worktree {
        result.push_str(&format!(
            "\n[worktree {}, branch {}]",
            wt.path.display(),
            wt.branch
        ));
    }
    Ok(TaskOutcome {
        answer: result,
        cost_usd,
        turns,
        summarised,
    })
}

/// One `Job::Summarize` call on its tier, recorded in the ledger like any
/// other request; `None` when the provider fails, so the caller falls
/// back to a cut.
async fn summarize(parent: &Session, text: &str, cap_tokens: usize) -> Option<String> {
    let tier = parent.config.jobs.tier_for(Job::Summarize);
    let tc = parent.config.tiers.get(tier);
    let model = ModelId(tc.model.clone());
    let req = Request {
        tier,
        job: Job::Summarize,
        model: model.clone(),
        system: vec![SystemBlock {
            text: format!(
                "Summarise the subagent result below in at most {cap_tokens} tokens. Keep \
                 file paths, line numbers, identifiers and exact error text; drop narration."
            ),
            cache: false,
        }],
        tools: vec![],
        messages: vec![Message {
            role: Role::User,
            content: vec![Content::Text {
                text: text.to_string(),
            }],
        }],
        effort: tc.effort,
        max_tokens: tc.max_tokens,
        thinking: tc.thinking,
        cache_breakpoints: vec![],
        stop_sequences: vec![],
    };
    let (tx, mut rx) = mpsc::channel(64);
    let provider = parent.provider.clone();
    let cancel = parent.cancel_token();
    let join = tokio::spawn(async move { provider.stream(req, tx, cancel).await });
    let mut out = String::new();
    while let Some(ev) = rx.recv().await {
        if let ProviderEvent::TextDelta { text } = ev {
            out.push_str(&text);
        }
    }
    let usage = join.await.ok()?.ok()?;
    parent
        .store
        .usage_insert(&cox_protocol::UsageRow {
            session_id: parent.id,
            turn: 0,
            job: Job::Summarize,
            tier,
            provider: parent.provider.id(),
            model,
            effort: Some(tc.effort),
            usage,
        })
        .ok()?;
    if budget::counts(tier, parent.config.budget.cheap_counts) {
        parent.add_spend(usage.cost_usd).await;
    }
    (!out.trim().is_empty()).then_some(out)
}

fn rank(r: Risk) -> u8 {
    match r {
        Risk::ReadOnly => 0,
        Risk::Write => 1,
        Risk::Exec => 2,
        Risk::Destructive => 3,
    }
}

pub(crate) fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    line.chars().take(60).collect()
}

fn core_error(e: CoreError) -> ToolError {
    ToolError::Denied {
        why: format!("subagent failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use cox_protocol::agent::AgentDef;

    use super::*;

    /// An `AgentTool` over a throwaway session, for `resolve`'s own claims
    /// (unit-level, no turn ever runs). T34.1 made `resolve`/`preset`
    /// resolution an instance method — it now reads discovered defs off
    /// the parent session — so this replaces the old bare-function call.
    fn test_agent_tool(defs: Vec<AgentDef>) -> AgentTool {
        let store = Arc::new(crate::MemoryStore::new());
        let provider =
            Arc::new(cox_provider::scripted::Scripted::from_toml("", "").expect("scripted"));
        let session = Session::new(
            cox_protocol::Config::default(),
            provider,
            vec![],
            store.clone(),
            store,
            PathBuf::from("/tmp/cox-subagent-unit"),
        )
        .expect("session");
        session.set_agent_defs(defs);
        AgentTool::new(session)
    }

    #[test]
    fn subagent_budget_is_a_slice_of_parent() {
        assert_eq!(slice(4.0, 0.0, None), 1.0, "a quarter by default");
        assert_eq!(slice(4.0, 3.5, None), 0.125);
        assert_eq!(slice(4.0, 1.0, Some(10.0)), 3.0, "never more than remains");
        assert_eq!(slice(4.0, 1.0, Some(0.5)), 0.5);
        assert_eq!(
            slice(1.0, 2.0, Some(1.0)),
            0.0,
            "nothing left, nothing granted"
        );
    }

    #[test]
    fn subagent_presets_are_explore_and_shell() {
        let tool = test_agent_tool(vec![]);
        let explore = tool.resolve(&json!({}));
        assert_eq!(
            explore.map(|p| (p.name, p.read_only)),
            Ok(("explore".to_string(), true))
        );
        let shell = tool.resolve(&json!({"preset": "shell"}));
        assert_eq!(
            shell.map(|p| (p.name, p.read_only)),
            Ok(("shell".to_string(), false))
        );
        assert!(tool.resolve(&json!({"preset": "nope"})).is_err());
    }

    /// T44.1: what the Engine concludes for an `explore` child with worktree
    /// isolation — the call the turn loop builds from `risk`/`subject`.
    fn worktree_isolation_outcome(mode: PermissionMode, allow: &[&str]) -> crate::Outcome {
        let tool = test_agent_tool(vec![]);
        let input = json!({"task": "look around", "isolation": "worktree"});
        let call = ToolCall {
            id: cox_protocol::CallId::new(),
            name: "agent".into(),
            risk: tool.risk(&input),
            subject: tool.subject(&input),
            input,
            segments: None,
        };
        let cfg = cox_protocol::config::PermissionsConfig {
            allow: allow.iter().map(|r| (*r).to_string()).collect(),
            ..Default::default()
        };
        let engine = crate::Engine::compile(&cfg, None, std::path::Path::new("/repo"))
            .expect("rules compile");
        engine.decide(
            &call,
            mode,
            cox_protocol::types::ApprovalPolicy::OnRequest,
            cox_protocol::types::SandboxMode::WorkspaceWrite,
            &[],
        )
    }

    #[test]
    fn worktree_isolation_asks_in_default_mode() {
        for mode in [PermissionMode::Default, PermissionMode::Auto] {
            assert_eq!(
                worktree_isolation_outcome(mode, &[]),
                crate::Outcome::Ask(Why::Risk {
                    risk: Risk::Destructive
                }),
                "{mode:?}"
            );
        }
    }

    #[test]
    fn worktree_isolation_is_denied_in_plan_mode() {
        assert!(matches!(
            worktree_isolation_outcome(PermissionMode::Plan, &[]),
            crate::Outcome::Deny { .. }
        ));
        let plain = test_agent_tool(vec![]).risk(&json!({"task": "x", "isolation": "none"}));
        assert_eq!(plain, Risk::ReadOnly, "no isolation keeps the tools' risk");
    }

    #[test]
    fn worktree_isolation_respects_an_allow_rule() {
        assert_eq!(
            worktree_isolation_outcome(PermissionMode::Default, &["agent(explore)"]),
            crate::Outcome::Allow {
                by: DecidedBy::Rule
            }
        );
    }

    /// T34.1: an unrecognised name is denied, and the error names both the
    /// built-in presets and whatever `.cox/agents`/`.claude/agents`
    /// discovered. Unit-level (like the test above), not a full turn: a
    /// failed `resolve` makes `risk()` fall back to `Exec` (pre-existing,
    /// unchanged behaviour — an unresolvable call could be anything), which
    /// would otherwise need an approval answer before `call()`'s own
    /// friendlier text ever surfaces as a tool result.
    #[test]
    fn agent_unknown_preset_lists_builtin_and_discovered_names_in_error() {
        let tool = test_agent_tool(vec![AgentDef {
            name: "reviewer".into(),
            description: "reviews a diff".into(),
            tools: vec!["read".into()],
            model: Some("haiku".into()),
            path: PathBuf::from("<test>/.cox/agents/reviewer.md"),
            body: "You review changes for correctness.".into(),
            disabled: false,
            permission_mode: None,
        }]);
        let err = tool
            .resolve(&json!({"preset": "nope"}))
            .expect_err("unknown preset");
        let ToolError::Denied { why } = err else {
            panic!("expected Denied, got {err:?}");
        };
        for name in ["explore", "shell", "reviewer"] {
            assert!(why.contains(name), "{name:?} missing from {why:?}");
        }
    }

    /// T34.10: `disabled: true` still discovers (the def reaches
    /// `set_agent_defs`, so `cox ext list` would still show it) but the
    /// `agent` tool neither offers it in its own description nor resolves
    /// it by name — matching OpenCode's `permission: deny` (research.md
    /// §4.3.7). The unknown-preset error also stays silent about it, since
    /// that error's own name list comes from `custom_names`.
    #[test]
    fn disabled_agent_def_is_discovered_but_not_offered_to_the_model() {
        let tool = test_agent_tool(vec![AgentDef {
            name: "blocked".into(),
            description: "not for the model".into(),
            tools: vec![],
            model: None,
            path: PathBuf::from("<test>/.cox/agents/blocked.md"),
            body: "internal only".into(),
            disabled: true,
            permission_mode: None,
        }]);
        assert!(
            !tool.spec().description.contains("blocked"),
            "{}",
            tool.spec().description
        );
        let err = tool
            .resolve(&json!({"preset": "blocked"}))
            .expect_err("disabled preset must not resolve");
        let ToolError::Denied { why } = err else {
            panic!("expected Denied, got {err:?}");
        };
        // `why` echoes the requested (invalid) name back once, so check
        // only the "available presets" list it also names — that list
        // must not offer `blocked` as something the model could retry.
        let available = why
            .split("available presets: ")
            .nth(1)
            .expect("error names the available presets");
        assert!(
            !available.contains("blocked"),
            "disabled name offered as available: {why:?}"
        );
    }

    use std::sync::{Mutex as StdMutex, OnceLock};
    use std::time::Duration;

    use cox_protocol::errors::ProviderError;
    use cox_protocol::traits::Provider;
    use cox_protocol::types::{Caps, PermissionMode, ProviderId, Usage};
    use cox_provider::scripted::Scripted;

    use crate::tasks::Child;

    /// The script, plus each request's tier and text parts, so a test can
    /// see exactly what history a child turn was sent with, and each
    /// request's system blocks joined, for what the child was told about
    /// itself.
    struct Recording {
        script: Scripted,
        seen: StdMutex<Vec<(Tier, Vec<String>)>>,
        system: StdMutex<Vec<(Tier, String)>>,
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
            let texts = req.messages.iter().flat_map(|m| &m.content);
            let texts = texts.filter_map(|c| match c {
                Content::Text { text } => Some(text.clone()),
                _ => None,
            });
            let row = (req.tier, texts.collect());
            self.seen.lock().expect("lock").push(row);
            let system = req.system.iter().map(|b| b.text.as_str());
            let system = (req.tier, system.collect::<Vec<_>>().join("\n"));
            self.system.lock().expect("lock").push(system);
            self.script.stream(req, sink, cancel).await
        }
        async fn count_tokens(&self, req: &Request) -> Result<u32, ProviderError> {
            self.script.count_tokens(req).await
        }
    }

    impl Recording {
        /// The child's requests: `explore` runs on the cheap tier, the
        /// parent on `code`.
        fn child(&self) -> Vec<Vec<String>> {
            let seen = self.seen.lock().expect("lock");
            let rows = seen.iter().filter(|(tier, _)| *tier == Tier::Cheap);
            rows.map(|(_, texts)| texts.clone()).collect()
        }
    }

    /// Mid-turn, submits a message to the parent's one child.
    struct Poke(Arc<OnceLock<Session>>);

    #[async_trait]
    impl Tool for Poke {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "poke".into(),
                description: "messages the running child".into(),
                input_schema: json!({"type": "object"}),
                deferred: false,
                risk: Risk::ReadOnly,
                concurrency: Concurrency::Parallel,
            }
        }
        fn subject(&self, _input: &Value) -> String {
            "poke".into()
        }
        async fn call(&self, _input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
            let parent = self.0.get().ok_or(ToolError::Io)?;
            let task = *parent
                .inner
                .lock()
                .await
                .children
                .keys()
                .next()
                .ok_or(ToolError::Io)?;
            let sub = Submission::TaskMessage {
                task,
                from: None,
                hop: 0,
                text: "ping".into(),
            };
            parent.submit(sub).await.map_err(|_| ToolError::Io)?;
            Ok(ToolOutput {
                text: "poked".into(),
                is_error: false,
                diff: None,
                structured: None,
            })
        }
    }

    fn parent_with(toml: &str) -> (Session, Arc<Recording>, mpsc::Receiver<Event>) {
        parent_in(toml, PermissionMode::Default)
    }

    /// `parent_with`, configured with permission mode `mode`.
    fn parent_in(
        toml: &str,
        mode: PermissionMode,
    ) -> (Session, Arc<Recording>, mpsc::Receiver<Event>) {
        let script = Scripted::from_toml(toml, "").expect("scenario");
        let (seen, system) = (StdMutex::new(Vec::new()), StdMutex::new(Vec::new()));
        let provider = Arc::new(Recording {
            script,
            seen,
            system,
        });
        let store = Arc::new(crate::MemoryStore::new());
        let slot = Arc::new(OnceLock::new());
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(Poke(slot.clone()))];
        let cwd = PathBuf::from("/tmp/cox-subagent-unit");
        let mut config = cox_protocol::Config::default();
        config.core.workspace_roots = vec![cwd.clone()];
        config.permissions.mode = mode;
        let session = Session::new(config, provider.clone(), tools, store.clone(), store, cwd)
            .expect("session");
        let _ = slot.set(session.clone());
        let rx = session.events().expect("events");
        (session, provider, rx)
    }

    async fn until(rx: &mut mpsc::Receiver<Event>, done: impl Fn(&Event) -> bool) -> Vec<Event> {
        let mut out = Vec::new();
        loop {
            let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
            let ev = ev.expect("event timeout").expect("stream closed");
            let stop = done(&ev);
            out.push(ev);
            if stop {
                return out;
            }
        }
    }

    /// One parent turn over `parent`'s script, drained to its `TurnDone`.
    async fn parent_turn(parent: &Session, rx: &mut mpsc::Receiver<Event>) -> Vec<Event> {
        let runner = parent.clone();
        let running = tokio::spawn(async move {
            let text = "go".to_string();
            let (attachments, confirm_think) = (vec![], false);
            let sub = Submission::UserTurn {
                text,
                attachments,
                confirm_think,
            };
            runner.submit(sub).await
        });
        let events = until(rx, |e| matches!(e, Event::TurnDone { .. })).await;
        running.await.expect("join").expect("turn");
        events
    }

    /// T45.1: a child takes the parent's live mode, not the configured
    /// one — after Shift+Tab to plan, an `auto`-configured parent spawns a
    /// plan-mode child. The child's volatile system block renders the mode
    /// its `Session::build` also seeds the engine with.
    #[tokio::test]
    async fn child_inherits_parent_live_plan_mode() {
        let toml = r#"
[[turn]]
tool_calls = [{ name = "agent", input = { task = "look around" } }]
[[turn]]
text = "found it"
[[turn]]
text = "done"
"#;
        let (parent, provider, mut rx) = parent_in(toml, PermissionMode::Auto);
        let mode = Submission::SetPermissionMode {
            mode: PermissionMode::Plan,
        };
        parent.submit(mode).await.expect("set mode");
        parent_turn(&parent, &mut rx).await;
        let system = provider.system.lock().expect("lock").clone();
        let child: Vec<_> = system.iter().filter(|(t, _)| *t == Tier::Cheap).collect();
        assert_eq!(child.len(), 1, "one child request: {system:?}");
        assert!(
            child[0].1.contains("permission_mode=Plan"),
            "{}",
            child[0].1
        );
    }

    /// A read-only `AgentDef` on the cheap tier that asks for `mode`.
    fn def_asking(name: &str, mode: PermissionMode) -> AgentDef {
        AgentDef {
            name: name.into(),
            description: "d".into(),
            tools: vec!["poke".into()],
            model: Some("haiku".into()),
            path: PathBuf::from(format!("<test>/.cox/agents/{name}.md")),
            body: "b".into(),
            disabled: false,
            permission_mode: Some(mode),
        }
    }

    /// One `agent` call to `preset` from `parent`: the parent's events and
    /// the child's (cheap-tier) volatile system block.
    async fn run_preset(
        parent: &Session,
        provider: &Recording,
        rx: &mut mpsc::Receiver<Event>,
    ) -> (Vec<Event>, String) {
        let events = parent_turn(parent, rx).await;
        let system = provider.system.lock().expect("lock").clone();
        let child: Vec<_> = system.iter().filter(|(t, _)| *t == Tier::Cheap).collect();
        assert_eq!(child.len(), 1, "one child request: {system:?}");
        (events, child[0].1.clone())
    }

    const PRESET_TURN: &str = r#"
[[turn]]
tool_calls = [{ name = "agent", input = { task = "look around", preset = "asker" } }]
[[turn]]
text = "found it"
[[turn]]
text = "done"
"#;

    /// T45.2: a def asking for `bypass` under a `default` parent runs as
    /// `default`, and the clamp is said once as an Info notice.
    #[tokio::test]
    async fn agent_permission_mode_never_widens_parent() {
        let (parent, provider, mut rx) = parent_in(PRESET_TURN, PermissionMode::Default);
        parent.set_agent_defs(vec![def_asking("asker", PermissionMode::Bypass)]);
        let (events, child) = run_preset(&parent, &provider, &mut rx).await;
        assert!(child.contains("permission_mode=Default"), "{child}");
        let clamps: Vec<_> = events
            .iter()
            .filter(|e| {
                matches!(e, Event::Notice { level: Level::Info, text }
                    if text == "`asker` asked for `bypass`, runs as `default`")
            })
            .collect();
        assert_eq!(clamps.len(), 1, "{events:?}");
    }

    /// T45.2: a def asking for `plan` under an `auto` parent runs in plan,
    /// with no clamp notice (narrowing is what the field is for).
    #[tokio::test]
    async fn agent_permission_mode_can_narrow_to_plan() {
        let (parent, provider, mut rx) = parent_in(PRESET_TURN, PermissionMode::Auto);
        parent.set_agent_defs(vec![def_asking("asker", PermissionMode::Plan)]);
        let (events, child) = run_preset(&parent, &provider, &mut rx).await;
        assert!(child.contains("permission_mode=Plan"), "{child}");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::Notice { text, .. } if text.contains("asked for"))),
            "{events:?}"
        );
    }

    #[tokio::test]
    async fn task_message_reaches_a_still_running_subagent_after_its_current_turn() {
        let toml = r#"
[[turn]]
text = "delegating"
tool_calls = [{ name = "agent", input = { task = "work", tools = ["poke"] } }]
[[turn]]
text = "poking"
tool_calls = [{ name = "poke", input = {} }]
[[turn]]
text = "first answer"
[[turn]]
text = "got ping"
[[turn]]
text = "done"
"#;
        let (parent, seen, mut rx) = parent_with(toml);
        let events = parent_turn(&parent, &mut rx).await;

        let results: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                Event::ToolCallDone { result, .. } => Some(result.visible.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(results, ["got ping"], "the child answered the message");
        assert!(events.iter().any(|e| matches!(
            e,
            Event::TaskMessage { from: None, hop: 0, text, .. } if text == "ping"
        )));
        let child = seen.child();
        assert_eq!(child.len(), 3, "poke, first answer, then the message turn");
        let ping = "[message from parent] ping";
        assert!(!child[1].iter().any(|t| t == ping), "never mid-turn");
        let last = &child[2];
        let at = last.iter().position(|t| t == ping).expect("delivered");
        assert_eq!(last[at - 1], "first answer", "after the whole turn");
    }

    /// Also the card's `resumed_subagent_keeps_its_parent_id_and_budget_slice`:
    /// the resumed request runs on the child's own tier, and the parked
    /// slice never grows.
    #[tokio::test]
    async fn task_message_to_a_finished_subagent_resumes_it_with_history_intact() {
        let toml = r#"
[[turn]]
text = "delegating"
tool_calls = [{ name = "agent", input = { task = "remember 42" } }]
[[turn]]
text = "noted"
[[turn]]
text = "done"
[[turn]]
text = "it was 42"
"#;
        let (parent, seen, mut rx) = parent_with(toml);
        let events = parent_turn(&parent, &mut rx).await;
        let task = events
            .iter()
            .find_map(|e| match e {
                Event::TaskCreated { task, .. } => Some(*task),
                _ => None,
            })
            .expect("task");
        let slice = match parent.inner.lock().await.children.get(&task) {
            Some(Child::Finished(d)) => d.spec.config.budget.session_usd,
            _ => panic!("a finished child is kept, not dropped"),
        };

        let sub = Submission::TaskMessage {
            task,
            from: None,
            hop: 0,
            text: "what number?".into(),
        };
        parent.submit(sub).await.expect("deliver");
        let woke = until(
            &mut rx,
            |e| matches!(e, Event::Notice { text, .. } if text.contains("finished")),
        )
        .await;
        assert!(woke.iter().any(|e| matches!(
            e,
            Event::TaskCreated { task: t, tier: Tier::Cheap, .. } if *t == task
        )));

        let child = seen.child();
        assert_eq!(
            child.len(),
            2,
            "the first run and the resumed one, both cheap"
        );
        assert_eq!(
            child[1],
            ["remember 42", "noted", "[message from parent] what number?"],
            "the resumed turn sees its whole history"
        );
        let history = parent.history().await;
        let pointer = history
            .iter()
            .flat_map(|m| &m.content)
            .any(|c| matches!(c, Content::Text { text } if text.contains("it was 42")));
        assert!(pointer, "the answer reaches the parent as a pointer line");
        match parent.inner.lock().await.children.get(&task) {
            Some(Child::Finished(d)) => {
                assert!(d.spec.config.budget.session_usd <= slice, "the same slice");
            }
            _ => panic!("parked again after the resumed run"),
        }
    }

    fn child_spec() -> Spec {
        Spec {
            config: cox_protocol::Config::default(),
            tools: vec![],
            job: Job::Explore,
            tier: Tier::Cheap,
            worktree: None,
            name: "explore-1".into(),
            preset_name: "explore".into(),
            result_cap_tokens: 1000,
            label: "explore: a".into(),
            external: None,
        }
    }

    #[tokio::test]
    async fn sibling_message_is_relayed_through_the_parent() {
        let (parent, _, mut rx) = parent_with("[[turn]]\ntext = \"sent\"\n");
        let (a, b) = (TaskId::new(), TaskId::new());
        parent.track_child(a).await;
        parent.track_child(b).await;
        let spec = child_spec();
        let child = spawn(&parent, a, &spec, None).expect("child");
        let events = child.events().expect("events");
        // The child claims a sender and a hop; the parent overrides both.
        for (to, text) in [(b, "hi b"), (a, "hi parent")] {
            let forged = Event::TaskMessage {
                task: to,
                from: None,
                hop: 99,
                text: text.into(),
            };
            child.emit(forged).await.expect("emit");
        }
        let (progress, _) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        let task = a;
        let mut io = RunIo {
            task,
            spec,
            events,
            cancel,
            progress,
        };
        let out = run_task(&parent, child, "a".into(), &mut io).await;
        assert_eq!(out.map(|o| o.answer).ok().as_deref(), Some("sent"));

        let relayed = Queued {
            from: Some(a),
            hop: 1,
            text: "hi b".into(),
        };
        assert_eq!(parent.next_queued(b).await, Some(relayed));
        let parent_line = format!("[message from task {a}] hi parent");
        let history = parent.history().await;
        let line = history
            .iter()
            .flat_map(|m| &m.content)
            .any(|c| matches!(c, Content::Text { text } if *text == parent_line));
        assert!(line, "to its own task id means to the parent");
        let seen = until(
            &mut rx,
            |e| matches!(e, Event::TaskMessage { task, .. } if *task == a),
        )
        .await;
        assert!(seen.iter().any(|e| matches!(
            e,
            Event::TaskMessage { task, from: Some(f), hop: 1, .. } if *task == b && *f == a
        )));
    }

    #[tokio::test]
    async fn hop_limit_stops_a_ping_pong() {
        let (parent, _, _rx) = parent_with("");
        let (a, b, c) = (TaskId::new(), TaskId::new(), TaskId::new());
        for t in [a, b, c] {
            parent.track_child(t).await;
        }
        let (mut from, mut to) = (a, b);
        let mut delivered = 0;
        for _ in 0..10 {
            relay(&parent, from, to, "ping".into())
                .await
                .expect("relay");
            let Some(next) = parent.next_queued(to).await else {
                break;
            };
            delivered += 1;
            assert_eq!(next.hop, delivered, "each relay is one hop further");
            std::mem::swap(&mut from, &mut to);
        }
        assert_eq!(delivered, MAX_HOPS, "A→B→A stops after MAX_HOPS relays");
        // An independent message, sent during a turn its task started, is hop 1.
        relay(&parent, c, b, "hello".into()).await.expect("relay");
        assert_eq!(parent.next_queued(b).await.map(|q| q.hop), Some(1));
    }

    /// Like `test_agent_tool`, but with a chosen `core.max_concurrent_subagents`
    /// (T34.2), for the cap tests below — they never let a child turn run,
    /// so the throwaway `Scripted` provider is never actually called.
    fn agent_tool_with_cap(cap: u32) -> AgentTool {
        let mut config = cox_protocol::Config::default();
        config.core.max_concurrent_subagents = cap;
        let store = Arc::new(crate::MemoryStore::new());
        let provider =
            Arc::new(cox_provider::scripted::Scripted::from_toml("", "").expect("scripted"));
        let session = Session::new(
            config,
            provider,
            vec![],
            store.clone(),
            store,
            PathBuf::from("/tmp/cox-subagent-unit"),
        )
        .expect("session");
        AgentTool::new(session)
    }

    /// A `ToolCx` good enough to drive `AgentTool::call`'s cap check, which
    /// returns before touching any of these fields for real (`resolve`
    /// hasn't even run yet on the denied path).
    fn test_cx(tool: &AgentTool) -> ToolCx {
        let (tx, _rx) = mpsc::channel(8);
        ToolCx {
            roots: tool.parent.config.core.workspace_roots.clone(),
            writable_roots: tool.parent.config.core.workspace_roots.clone(),
            cwd: tool.parent.cwd.clone(),
            sandbox: cox_protocol::types::SandboxPolicy {
                mode: tool.parent.config.sandbox.mode,
                network: tool.parent.config.sandbox.network,
                writable: tool.parent.config.sandbox.writable.clone(),
                readonly_in_workspace: tool.parent.config.sandbox.readonly_in_workspace.clone(),
                linux_backend: tool.parent.config.sandbox.linux_backend,
            },
            archive: Arc::new(crate::MemoryStore::new()),
            cancel: CancellationToken::new(),
            output: tx,
            session: tool.parent.id,
            call: cox_protocol::ids::CallId::new(),
            agent: None,
            preset: None,
            relay: None,
        }
    }

    /// T34.2: over the cap, `call()` denies before spawning a child at all,
    /// naming both the cap and how many slots are already reserved — the
    /// same shape as the "unknown preset" denial above. The slots are
    /// taken directly via `try_reserve_agent_slot` (not a full `call()`
    /// each), so this is the narrowest possible proof of the check itself;
    /// `agent_calls_reserve_exactly_cap_slots_under_a_parallel_burst` below
    /// proves it holds under real concurrent `call()`s too.
    #[tokio::test]
    async fn agent_call_denied_when_concurrent_cap_reached() {
        let tool = agent_tool_with_cap(2);
        let _a = tool.parent.try_reserve_agent_slot(2).expect("slot 1 of 2");
        let _b = tool.parent.try_reserve_agent_slot(2).expect("slot 2 of 2");

        let err = tool
            .call(json!({"task": "one too many"}), &test_cx(&tool))
            .await
            .expect_err("cap reached");
        let ToolError::Denied { why } = err else {
            panic!("expected Denied, got {err:?}");
        };
        assert!(why.contains("cap"), "{why:?}");
        assert!(
            why.contains("2 of 2"),
            "names the cap and running count: {why:?}"
        );
    }

    /// T34.2: dropping a reservation guard (a running task completing,
    /// erroring, or being cancelled all drop it the same way) frees the
    /// slot — the very next call no longer trips the cap check. Proven by
    /// handing it an invalid preset instead of a working one: a cap denial
    /// would still say "cap reached"; `resolve()`'s own "unknown preset"
    /// text proves the call got past the cap check this time.
    #[tokio::test]
    async fn agent_call_allowed_after_a_running_task_completes() {
        let tool = agent_tool_with_cap(1);
        let holding = tool.parent.try_reserve_agent_slot(1).expect("the one slot");

        let full = tool
            .call(json!({"task": "denied while full"}), &test_cx(&tool))
            .await
            .expect_err("cap reached");
        assert!(
            matches!(full, ToolError::Denied { ref why } if why.contains("cap")),
            "{full:?}"
        );

        drop(holding); // the guard's `Drop` frees the slot, as a finished task's would
        let after = tool
            .call(
                json!({"task": "past the cap", "preset": "nope"}),
                &test_cx(&tool),
            )
            .await
            .expect_err("unknown preset, not a cap denial");
        let ToolError::Denied { why } = after else {
            panic!("expected Denied, got {after:?}");
        };
        assert!(
            why.contains("unknown agent preset"),
            "cap should no longer be the blocker: {why:?}"
        );
    }

    /// Like `agent_tool_with_cap`, but with `cap` scripted turns so the
    /// exactly-`cap` background children the burst test below lets past
    /// the reservation can each finish their one-shot child turn (no tool
    /// calls, so one scripted answer ends it) instead of erroring against
    /// an empty script; the child gets no tools of its own (`vec![]`,
    /// like `agent_tool_with_cap`), so it never needs one.
    fn agent_tool_with_cap_and_turns(cap: u32) -> AgentTool {
        let mut config = cox_protocol::Config::default();
        config.core.max_concurrent_subagents = cap;
        let toml: String = (0..cap)
            .map(|i| format!("[[turn]]\ntext = \"ok {i}\"\n\n"))
            .collect();
        let store = Arc::new(crate::MemoryStore::new());
        let provider =
            Arc::new(cox_provider::scripted::Scripted::from_toml(&toml, "").expect("scripted"));
        let session = Session::new(
            config,
            provider,
            vec![],
            store.clone(),
            store,
            PathBuf::from("/tmp/cox-subagent-unit"),
        )
        .expect("session");
        AgentTool::new(session)
    }

    /// T34.2 review: the card this fixes is exactly "a burst of parallel
    /// `agent` calls in one turn" — `turn.rs` dispatches
    /// `Concurrency::Parallel` tools (which `agent` is) concurrently via
    /// its own `JoinSet`, so a count-then-register cap check races: ten
    /// calls checked against a cap of eight can all read "0 running" before
    /// any of them registers. Ten real concurrent `call()`s over a cap of
    /// three must let exactly three through and deny the other seven, on
    /// every run, however the runtime happens to interleave them.
    #[tokio::test]
    async fn agent_calls_reserve_exactly_cap_slots_under_a_parallel_burst() {
        let cap = 3u32;
        let n = 10usize;
        let tool = Arc::new(agent_tool_with_cap_and_turns(cap));
        let mut handles = Vec::with_capacity(n);
        for i in 0..n {
            let tool = tool.clone();
            handles.push(tokio::spawn(async move {
                let cx = test_cx(&tool);
                tool.call(
                    json!({"task": format!("task {i}"), "background": true}),
                    &cx,
                )
                .await
            }));
        }
        let (mut ok, mut denied) = (0usize, 0usize);
        for h in handles {
            match h.await.expect("join") {
                Ok(_) => ok += 1,
                Err(ToolError::Denied { why }) if why.contains("cap") => denied += 1,
                Err(other) => panic!("unexpected error: {other:?}"),
            }
        }
        assert_eq!(ok, cap as usize, "exactly the cap's worth of calls succeed");
        assert_eq!(denied, n - cap as usize, "the rest are denied by the cap");
    }

    /// T34.6: the same routing `sibling_message_is_relayed_through_the_parent`
    /// proves, but through `send_message`'s own `Relay` impl instead of a
    /// forged `Event::TaskMessage` — proving the child branch resolves
    /// `"parent"` to its own task id (so `relay`'s `to == from` check
    /// fires) and a sibling's `TaskId` on its own, with no registry access.
    #[tokio::test]
    async fn sibling_message_is_relayed_through_the_parent_session() {
        let (parent, _, mut rx) = parent_with("[[turn]]\ntext = \"sent\"\n");
        let (a, b) = (TaskId::new(), TaskId::new());
        parent.track_child(a).await;
        parent.track_child(b).await;
        let spec = child_spec();
        let child = spawn(&parent, a, &spec, None).expect("child");
        let events = child.events().expect("events");
        child
            .send_message(&b.to_string(), "hi b")
            .await
            .expect("send to sibling by task id");
        child
            .send_message("parent", "hi parent")
            .await
            .expect("\"parent\" resolves to the child's own task id");
        let (progress, _) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        let task = a;
        let mut io = RunIo {
            task,
            spec,
            events,
            cancel,
            progress,
        };
        let out = run_task(&parent, child, "a".into(), &mut io).await;
        assert_eq!(out.map(|o| o.answer).ok().as_deref(), Some("sent"));

        let relayed = Queued {
            from: Some(a),
            hop: 1,
            text: "hi b".into(),
        };
        assert_eq!(parent.next_queued(b).await, Some(relayed));
        let parent_line = format!("[message from task {a}] hi parent");
        let history = parent.history().await;
        let line = history
            .iter()
            .flat_map(|m| &m.content)
            .any(|c| matches!(c, Content::Text { text } if *text == parent_line));
        assert!(
            line,
            "\"parent\" routed to the parent, not queued as a sibling"
        );
        let seen = until(
            &mut rx,
            |e| matches!(e, Event::TaskMessage { task, .. } if *task == a),
        )
        .await;
        assert!(seen.iter().any(|e| matches!(
            e,
            Event::TaskMessage { task, from: Some(f), hop: 1, .. } if *task == b && *f == a
        )));
    }

    /// T34.9: `spawn_child` shares the parent's name→`TaskId` registry with
    /// each child (`Session::task_names`), so a child's own `send_message`
    /// resolves a sibling by its deterministic registry name (`talker-2`)
    /// — the only address a scripted scenario can know ahead of time,
    /// since `TaskId` is random and never appears in a scenario file.
    #[tokio::test]
    async fn child_resolves_a_sibling_by_name() {
        let (parent, _, _rx) = parent_with("[[turn]]\ntext = \"sent\"\n");
        let (a, b) = (TaskId::new(), TaskId::new());
        parent.track_child(a).await;
        parent.track_child(b).await;
        parent.name_task("talker-2".into(), b).await;
        let spec = child_spec();
        let child = spawn(&parent, a, &spec, None).expect("child");
        let events = child.events().expect("events");

        child
            .send_message("talker-2", "hi b")
            .await
            .expect("child resolves a sibling by its registry name");

        // `run_task` drains the child's own turn and, along the way,
        // relays the `Event::TaskMessage` it just emitted into the
        // parent's `Submission::TaskMessage` (`relay`, below) — the same
        // path `sibling_message_is_relayed_through_the_parent_session`
        // exercises for a literal `TaskId`.
        let (progress, _) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        let mut io = RunIo {
            task: a,
            spec,
            events,
            cancel,
            progress,
        };
        let out = run_task(&parent, child, "a".into(), &mut io).await;
        assert_eq!(out.map(|o| o.answer).ok().as_deref(), Some("sent"));

        let relayed = Queued {
            from: Some(a),
            hop: 1,
            text: "hi b".into(),
        };
        assert_eq!(parent.next_queued(b).await, Some(relayed));
    }

    /// A `ToolCx` carrying `relay: Some(Arc::new(session.clone()))`, exactly
    /// what `cox-core/src/turn.rs`'s `run_one` stamps into each call's own
    /// context — never a handle fixed once and shared.
    fn cx_with_relay(session: &Session) -> ToolCx {
        let (tx, _rx) = mpsc::channel(8);
        ToolCx {
            roots: session.config.core.workspace_roots.clone(),
            writable_roots: session.config.core.workspace_roots.clone(),
            cwd: session.cwd.clone(),
            sandbox: cox_protocol::types::SandboxPolicy {
                mode: session.config.sandbox.mode,
                network: session.config.sandbox.network,
                writable: session.config.sandbox.writable.clone(),
                readonly_in_workspace: session.config.sandbox.readonly_in_workspace.clone(),
                linux_backend: session.config.sandbox.linux_backend,
            },
            archive: Arc::new(crate::MemoryStore::new()),
            cancel: CancellationToken::new(),
            output: tx,
            session: session.id,
            call: cox_protocol::ids::CallId::new(),
            agent: None,
            preset: None,
            relay: Some(Arc::new(session.clone()) as Arc<dyn Relay>),
        }
    }

    /// T34.6 review: the bug a shared `DeferredRelay` had — one relay handle
    /// pointing at whichever session built it first — must not resurface
    /// now that the binding is per call. Two siblings' own `ToolCx`s (built
    /// the same way `run_one` builds one) must each reach *their own*
    /// session's `Relay` impl: a call through child a's `ToolCx.relay` is
    /// attributed to a on a's own event stream, never to b or to a
    /// parent-side `deliver` that bypasses the child branch entirely.
    #[tokio::test]
    async fn each_childs_tool_cx_relay_is_bound_to_its_own_session() {
        let (parent, _, _rx) = parent_with("[[turn]]\ntext = \"sent\"\n");
        let (a, b) = (TaskId::new(), TaskId::new());
        parent.track_child(a).await;
        parent.track_child(b).await;
        let child_a = spawn(&parent, a, &child_spec(), None).expect("child a");
        let child_b = spawn(&parent, b, &child_spec(), None).expect("child b");
        let mut events_a = child_a.events().expect("a's events");
        let mut events_b = child_b.events().expect("b's events");

        let cx_a = cx_with_relay(&child_a);
        let cx_b = cx_with_relay(&child_b);
        cx_a.relay
            .as_ref()
            .expect("a's ToolCx carries a relay")
            .send_message("parent", "hi from a")
            .await
            .expect("a's own relay resolves \"parent\" to a's own task id");
        cx_b.relay
            .as_ref()
            .expect("b's ToolCx carries a relay")
            .send_message("parent", "hi from b")
            .await
            .expect("b's own relay resolves \"parent\" to b's own task id");

        // Each child's own stream opens with `SessionStarted`; drain to the
        // `TaskMessage` the call above should have produced on *that*
        // stream specifically.
        let seen_a = until(&mut events_a, |e| matches!(e, Event::TaskMessage { .. })).await;
        assert!(
            seen_a.iter().any(|e| matches!(
                e,
                Event::TaskMessage { task, text, .. } if *task == a && text == "hi from a"
            )),
            "a's call must land on a's own stream, attributed to a: {seen_a:?}"
        );
        let seen_b = until(&mut events_b, |e| matches!(e, Event::TaskMessage { .. })).await;
        assert!(
            seen_b.iter().any(|e| matches!(
                e,
                Event::TaskMessage { task, text, .. } if *task == b && text == "hi from b"
            )),
            "b's call must land on b's own stream, attributed to b: {seen_b:?}"
        );
    }

    /// SM§5: the 17th message received by one task in `Session::deliver`
    /// (`tasks.rs`) is refused, shaped like T34.2's concurrency-cap denial,
    /// rather than queued forever behind a child that never catches up.
    #[tokio::test]
    async fn message_cap_denies_the_nth_plus_one_follow_up() {
        let (parent, _, _rx) = parent_with("");
        let task = TaskId::new();
        parent.track_child(task).await;
        for i in 0..MAX_MESSAGES_PER_TASK {
            parent
                .deliver(task, None, 0, format!("msg {i}"))
                .await
                .unwrap_or_else(|e| {
                    panic!("message {i} of {MAX_MESSAGES_PER_TASK} should be admitted: {e}")
                });
        }
        let err = parent
            .deliver(task, None, 0, "one too many".into())
            .await
            .expect_err("the 17th message trips the flood cap");
        let CoreError::Denied { why } = err else {
            panic!("expected Denied, got {err:?}");
        };
        assert!(why.contains("cap"), "{why:?}");
        assert!(
            why.contains(&format!(
                "{MAX_MESSAGES_PER_TASK} of {MAX_MESSAGES_PER_TASK}"
            )),
            "names the cap: {why:?}"
        );
    }

    /// T34.6 point 4: waking a *dormant* child (`Session::deliver`'s
    /// `Finished` branch, `tasks.rs`) reserves a T34.2 slot for the woken
    /// run, same as a fresh `agent` call — at the cap the sender is denied
    /// and, unlike the flood cap above, the message is never queued at
    /// all, since there is no running turn left for it to wait behind.
    #[tokio::test]
    async fn waking_a_dormant_child_at_the_cap_is_denied() {
        let tool = agent_tool_with_cap(1);
        let parent = tool.parent.clone();
        let task = TaskId::new();
        parent.track_child(task).await;
        let dormant = Dormant {
            session: SessionId::new(),
            spec: child_spec(),
        };
        assert!(
            parent.park_child(task, dormant).await.is_none(),
            "nothing queued yet, so the child goes dormant"
        );

        // Fill the one slot the cap allows, so waking has nowhere to run.
        let _holding = parent.try_reserve_agent_slot(1).expect("the one slot");

        let err = parent
            .deliver(task, None, 0, "wake up".into())
            .await
            .expect_err("no slot left to run the woken child");
        let CoreError::Denied { why } = err else {
            panic!("expected Denied, got {err:?}");
        };
        assert!(why.contains("cap"), "{why:?}");
        assert!(
            matches!(
                parent.inner.lock().await.children.get(&task),
                Some(Child::Finished(_))
            ),
            "denied, so the message is not queued: still dormant, not woken"
        );
    }

    /// Stands in for the host's CLI driver (T35.2 spawns the real one): each
    /// turn records its prompt and answers `<name> did: <prompt>`; with a
    /// parent in `poke`, its first turn messages its own task mid-run, the
    /// way T34.5's `Poke` tool does from a model child.
    struct FakeAgent {
        prompts: StdMutex<Vec<String>>,
        parent: OnceLock<Session>,
        poke: bool,
        reported: Option<Usage>,
    }

    #[async_trait]
    impl ExternalAgent for FakeAgent {
        fn name(&self) -> &str {
            "cursor"
        }
        async fn turn(
            &self,
            turn: cox_protocol::ids::TurnId,
            prompt: String,
            events: mpsc::Sender<Event>,
            _cancel: CancellationToken,
        ) -> Result<Option<Usage>, CoreError> {
            let first = {
                let mut prompts = self.prompts.lock().expect("lock");
                prompts.push(prompt.clone());
                prompts.len() == 1
            };
            if let (true, true, Some(parent)) = (first, self.poke, self.parent.get()) {
                let task = *parent
                    .inner
                    .lock()
                    .await
                    .children
                    .keys()
                    .next()
                    .expect("child");
                let text = "ping".to_string();
                let sub = Submission::TaskMessage {
                    task,
                    from: None,
                    hop: 0,
                    text,
                };
                parent.submit(sub).await?;
            }
            let item = ItemId::new();
            let text = format!("cursor did: {prompt}");
            let kind = cox_protocol::types::ItemKind::AssistantMessage { text };
            let _ = events.send(Event::ItemStarted { item, kind }).await;
            let _ = events.send(Event::ItemDone { item }).await;
            let stop = cox_protocol::types::StopReason::EndTurn;
            let _ = events.send(Event::TurnDone { turn, stop }).await;
            Ok(self.reported)
        }
    }

    /// A parent with `agent` granted as the one external agent, in bypass
    /// mode: an external preset is `Exec` risk, and approval is not the claim.
    async fn external_parent(
        toml: &str,
        agent: Arc<FakeAgent>,
    ) -> (Session, Arc<crate::MemoryStore>, mpsc::Receiver<Event>) {
        let provider = Arc::new(Scripted::from_toml(toml, "").expect("scenario"));
        let store = Arc::new(crate::MemoryStore::new());
        let cwd = PathBuf::from("/tmp/cox-subagent-unit");
        let mut config = cox_protocol::Config::default();
        config.core.workspace_roots = vec![cwd.clone()];
        let session = Session::new(config, provider, vec![], store.clone(), store.clone(), cwd)
            .expect("session");
        let _ = agent.parent.set(session.clone());
        session.set_external_agents(vec![agent]);
        let rx = session.events().expect("events");
        let mode = cox_protocol::types::PermissionMode::Bypass;
        let sub = Submission::SetPermissionMode { mode };
        session.submit(sub).await.expect("mode");
        (session, store, rx)
    }

    fn fake(poke: bool, reported: Option<Usage>) -> Arc<FakeAgent> {
        Arc::new(FakeAgent {
            prompts: StdMutex::new(Vec::new()),
            parent: OnceLock::new(),
            poke,
            reported,
        })
    }

    fn tool_results(events: &[Event]) -> Vec<String> {
        let done = events.iter().filter_map(|e| match e {
            Event::ToolCallDone { result, .. } => Some(result.visible.clone()),
            _ => None,
        });
        done.collect()
    }

    const DISPATCH: &str = r#"
[[turn]]
text = "delegating"
tool_calls = [{ name = "agent", input = { task = "work", preset = "cursor" } }]
[[turn]]
text = "done"
"#;

    #[tokio::test]
    async fn agent_dispatches_a_granted_external_agent_preset_by_name() {
        let agent = fake(false, None);
        let (parent, store, mut rx) = external_parent(DISPATCH, agent.clone()).await;
        let tool = AgentTool::new(parent.clone());
        assert!(
            tool.spec().description.contains("cursor"),
            "offered to the model"
        );

        let events = parent_turn(&parent, &mut rx).await;
        assert_eq!(tool_results(&events), ["cursor did: work"]);
        assert_eq!(*agent.prompts.lock().expect("lock"), ["work"]);
        let rows = store.usage_rows();
        let row = rows.iter().find(|r| r.provider == ProviderId::External);
        let row = row.expect("the external turn has a usage row");
        assert_eq!((row.usage.input_tokens, row.usage.output_tokens), (0, 0));
    }

    #[tokio::test]
    async fn task_message_reaches_a_running_external_agent_task() {
        let agent = fake(true, None);
        let (parent, _, mut rx) = external_parent(DISPATCH, agent.clone()).await;
        let events = parent_turn(&parent, &mut rx).await;

        let ping = "[message from parent] ping";
        assert_eq!(tool_results(&events), [format!("cursor did: {ping}")]);
        assert_eq!(
            *agent.prompts.lock().expect("lock"),
            ["work", ping],
            "delivered as a whole new turn after the running one"
        );
    }

    #[tokio::test]
    async fn external_agent_turn_writes_a_billed_externally_usage_row() {
        let reported = Usage {
            input_tokens: 7,
            output_tokens: 3,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            estimated: false,
            cost_usd: 9.5,
            latency_ms: 0,
        };
        let (parent, store, mut rx) = external_parent(DISPATCH, fake(false, Some(reported))).await;
        parent_turn(&parent, &mut rx).await;

        let rows = store.usage_rows();
        let external: Vec<_> = rows
            .iter()
            .filter(|r| r.provider == ProviderId::External)
            .collect();
        assert_eq!(external.len(), 1, "one row per external turn: {rows:?}");
        let row = external[0];
        assert_eq!(row.model.0, "cursor");
        assert_eq!(row.job, Job::Agent);
        assert_eq!(
            (row.usage.input_tokens, row.usage.output_tokens),
            (7, 3),
            "reported tokens kept"
        );
        assert_eq!(
            row.usage.cost_usd, 0.0,
            "never priced: billed on the user's plan"
        );
    }
}
