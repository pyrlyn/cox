// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! One provider call and its tool batch. Kept separate from `Session` so
//! the loop's `step` stays a state transition, not a grab-bag of I/O.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use cox_protocol::ArchivePut;
use cox_protocol::errors::{CoreError, ToolError};
use cox_protocol::ids::{CallId, ItemId, TurnId};
use cox_protocol::image;
use cox_protocol::traits::{Relay, Tool, ToolCx};
use cox_protocol::types::{
    Attachment, Concurrency, Content, DecidedBy, Decision, Event, HookEvent, HookOutcome, ItemKind,
    Level, Message, ModelId, Risk, Role, SandboxMode, SandboxPolicy, Source, StopReason, ToolCall,
    ToolOutput, ToolResult, Usage, Why,
};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tracing::Instrument as _;

use crate::checkpoint;
use crate::hooks;
use crate::permission::Outcome;
use crate::permission::policy::{ExecPath, exec_path};
use crate::session::Session;

tokio::task_local! {
    /// Who a call runs for when it is not the model: `plugin <id>` for a
    /// plugin's `cox_invoke_tool` (T33.13). Task-local so the one tool path
    /// (`run_tools` → `gate` → `ask`) needs no second entry point; `ask`
    /// shows it as `Source.agent` ("plugin <id> asks"). A call `run_tools`
    /// spawns in parallel is read-only and never asks, so losing it there
    /// is harmless.
    pub(crate) static ORIGIN: String;
}

#[derive(Default)]
pub(crate) struct Streamed {
    pub text: String,
    pub thinking: String,
    pub calls: Vec<(CallId, String, Value)>,
    /// Thought signatures by call id (T39.1); opaque, for replay only.
    pub signatures: HashMap<CallId, String>,
    pub usage: Option<Usage>,
    pub response_model: Option<ModelId>,
    pub stop: Option<StopReason>,
    pub retries: u32,
    pub retry_delay_ms: u64,
}

struct Acc {
    id: CallId,
    name: String,
    input: String,
}

/// The streamed `Thinking` item still growing (A91), with when its first
/// and latest deltas arrived: `ThinkingDone` reports the gap between them.
struct Thought {
    item: ItemId,
    first: Instant,
    last: Instant,
}

/// Closes a streamed thought: its duration, then its `ItemDone`.
async fn end_thought(session: &Session, thought: Thought) -> Result<(), CoreError> {
    let duration_ms = thought.last.duration_since(thought.first).as_millis() as u64;
    session
        .emit(Event::ThinkingDone {
            item: thought.item,
            duration_ms,
        })
        .await?;
    session.emit(Event::ItemDone { item: thought.item }).await
}

/// Forwards `ProviderEvent`s as `Event`s and collects tool-use blocks.
///
/// Reasoning streams into its own `Thinking` item (A91), and the reply's
/// `AssistantMessage` item starts only once the thought is over — before
/// the first other provider event, or at the end — so every surface lists
/// the thought ahead of the reply it led to.
pub(crate) async fn consume_provider(
    session: &Session,
    rx: &mut mpsc::Receiver<cox_protocol::types::ProviderEvent>,
    assistant_item: ItemId,
) -> Result<Streamed, CoreError> {
    use cox_protocol::types::ProviderEvent as P;
    let mut out = Streamed::default();
    let mut current: Option<Acc> = None;
    let mut thought: Option<Thought> = None;
    let mut replying = false;
    while let Some(ev) = rx.recv().await {
        if !matches!(ev, P::ThinkingDelta { .. }) {
            if let Some(done) = thought.take() {
                end_thought(session, done).await?;
            }
            if !replying {
                replying = true;
                start_reply(session, assistant_item).await?;
            }
        }
        match ev {
            P::MessageStart { model } => out.response_model = Some(model),
            P::TextDelta { text } => {
                out.text.push_str(&text);
                session
                    .emit(Event::TextDelta {
                        item: assistant_item,
                        text,
                    })
                    .await?;
            }
            P::ThinkingDelta { text } => {
                if crate::session::capture_message_content() {
                    out.thinking.push_str(&text);
                }
                let now = Instant::now();
                let item = match thought.as_mut() {
                    Some(open) => {
                        open.last = now;
                        open.item
                    }
                    None => {
                        let item = ItemId::new();
                        let kind = ItemKind::Thinking {
                            text: String::new(),
                            signature: None,
                        };
                        session.emit(Event::ItemStarted { item, kind }).await?;
                        thought = Some(Thought {
                            item,
                            first: now,
                            last: now,
                        });
                        item
                    }
                };
                session.emit(Event::ThinkingDelta { item, text }).await?;
            }
            P::ToolUseStart { id, name } => {
                current = Some(Acc {
                    id,
                    name,
                    input: String::new(),
                });
            }
            P::ToolUseSignature { signature } => {
                if let Some(acc) = current.as_ref() {
                    out.signatures.insert(acc.id, signature);
                }
            }
            P::ToolUseInputDelta { text } => {
                if let Some(acc) = current.as_mut() {
                    acc.input.push_str(&text);
                }
            }
            P::ToolUseEnd => {
                if let Some(acc) = current.take() {
                    let input = serde_json::from_str(&acc.input).unwrap_or(Value::Null);
                    out.calls.push((acc.id, acc.name, input));
                }
            }
            P::Stop { stop } => out.stop = Some(stop),
            P::Usage { usage } => out.usage = Some(usage),
            P::Retrying { attempt, after_ms } => {
                out.retries = out.retries.max(attempt);
                out.retry_delay_ms = out.retry_delay_ms.saturating_add(after_ms);
                tracing::warn!(
                    event.name = "cox.provider.retrying",
                    retry.attempt = attempt,
                    retry.after_ms = after_ms,
                    "provider request retrying"
                );
            }
            P::Error { error } => {
                tracing::error!(
                    event.name = "cox.provider.stream_error",
                    error = %error,
                    "provider stream failed"
                );
                return Err(CoreError::Provider { error });
            }
        }
    }
    if let Some(done) = thought {
        end_thought(session, done).await?;
    }
    if !replying {
        start_reply(session, assistant_item).await?;
    }
    Ok(out)
}

/// Opens the reply's (still empty) `AssistantMessage` item.
async fn start_reply(session: &Session, item: ItemId) -> Result<(), CoreError> {
    let kind = ItemKind::AssistantMessage {
        text: String::new(),
    };
    session.emit(Event::ItemStarted { item, kind }).await
}

/// Runs one batch of tool calls; results are returned in emission order.
pub(crate) async fn run_tools(
    session: &Session,
    turn: TurnId,
    calls: Vec<(CallId, String, Value)>,
) -> Result<Vec<(CallId, ToolResult)>, CoreError> {
    run_signed_tools(session, turn, calls, &HashMap::new()).await
}

/// [`run_tools`] for a model batch whose calls may carry thought signatures
/// (T39.2): a signed call's `ToolCallRequested` is preceded by an empty
/// signed `Thinking` item, so the rollout rebuilds the block in the same
/// place the live history put it (§1.15 invariant 6).
pub(crate) async fn run_signed_tools(
    session: &Session,
    turn: TurnId,
    calls: Vec<(CallId, String, Value)>,
    signatures: &HashMap<CallId, String>,
) -> Result<Vec<(CallId, ToolResult)>, CoreError> {
    let tools: HashMap<String, Arc<dyn Tool>> = session
        .tools
        .iter()
        .map(|t| (t.spec().name, t.clone()))
        .collect();
    let order: Vec<CallId> = calls.iter().map(|(id, _, _)| *id).collect();
    let calls: Vec<ToolCall> = calls
        .into_iter()
        .map(|(id, name, input)| {
            let tool = tools.get(&name);
            ToolCall {
                id,
                subject: tool.map(|t| t.subject(&input)).unwrap_or_default(),
                segments: tool.and_then(|t| t.segments(&input)),
                // Per call, not per tool: `apply_patch` escalates to
                // `Destructive` on the patches that delete a lot of files.
                risk: tool.map(|t| t.risk(&input)).unwrap_or(Risk::ReadOnly),
                name,
                input,
            }
        })
        .collect();
    for call in &calls {
        if let Some(signature) = signatures.get(&call.id) {
            let item = ItemId::new();
            let kind = ItemKind::Thinking {
                text: String::new(),
                signature: Some(signature.clone()),
            };
            session.emit(Event::ItemStarted { item, kind }).await?;
            session.emit(Event::ItemDone { item }).await?;
        }
        session
            .emit(Event::ToolCallRequested { call: call.clone() })
            .await?;
    }
    // Gate serially so the user answers one prompt at a time and an
    // `AllowForSession` grant covers the calls behind it in the same batch.
    let mut serial = Vec::new();
    let mut parallel = Vec::new();
    let mut done = HashMap::new();
    for call in calls {
        let Some(tool) = tools.get(&call.name).cloned() else {
            done.insert(
                call.id,
                failed_result(&format!("unknown tool {}", call.name)),
            );
            continue;
        };
        let id = call.id;
        let call = match gate(session, tool.as_ref(), call).await? {
            Ok(call) => call,
            Err(result) => {
                done.insert(id, result);
                continue;
            }
        };
        session.dedup_invalidate(call.risk, &call.subject).await;
        let needs_snapshot = call.risk != Risk::ReadOnly && tool.touches(&call.input).is_none();
        if tool.spec().concurrency == Concurrency::Exclusive || needs_snapshot {
            serial.push((id, tool, call.input));
        } else {
            parallel.push((id, tool, call.input));
        }
    }
    for (id, tool, input) in serial {
        let (id, result) = run_one(session, turn, id, tool, input).await;
        done.insert(id, result);
    }
    let cap = session.config.core.parallel_tools.max(1) as usize;
    let mut set = JoinSet::new();
    let mut inflight = 0usize;
    let mut rest = parallel.into_iter();
    loop {
        while inflight < cap {
            let Some((id, tool, input)) = rest.next() else {
                break;
            };
            let session = session.clone_handle();
            let parent = tracing::Span::current();
            set.spawn(
                async move { run_one(&session, turn, id, tool, input).await }.instrument(parent),
            );
            inflight += 1;
        }
        let Some(joined) = set.join_next().await else {
            break;
        };
        inflight -= 1;
        match joined {
            Ok((id, result)) => {
                done.insert(id, result);
            }
            Err(_) => return Err(CoreError::Interrupted),
        }
    }
    let results: Vec<(CallId, ToolResult)> = order
        .into_iter()
        .map(|id| {
            let result = done
                .remove(&id)
                .unwrap_or_else(|| failed_result("tool did not return"));
            (id, result)
        })
        .collect();
    for (id, result) in &results {
        session
            .emit(Event::ToolCallDone {
                call_id: *id,
                result: result.clone(),
            })
            .await?;
    }
    Ok(results)
}

/// Asks the permission engine and, when it escalates, the user. Returns the
/// call to run (the user may have edited its input) or the failed result the
/// model sees. Auto-allows emit nothing: they are the common case and the
/// rollout already carries `ToolCallRequested`.
async fn gate(
    session: &Session,
    tool: &dyn Tool,
    mut call: ToolCall,
) -> Result<Result<ToolCall, ToolResult>, CoreError> {
    let id = call.id;
    let denied = |reason: &str| failed_result(&format!("permission denied: {reason}"));
    // §1.8 step i: `PreToolUse` runs before the engine so a rewritten input
    // is what the rules judge.
    let payload = serde_json::json!({ "tool_name": call.name, "tool_input": call.input });
    match hooks::fire(session, HookEvent::PreToolUse, payload).await {
        HookOutcome::Block { reason } => {
            return Ok(Err(failed_result(&format!("blocked by hook: {reason}"))));
        }
        HookOutcome::Modify { input } => rate(&mut call, tool, input),
        _ => {}
    }
    // T33.21: `risk` advice may only raise what the engine judges next.
    call = session.advise_risk(call).await?;
    loop {
        let why = match session.decide(&call).await {
            Outcome::Allow { .. } => return Ok(Ok(call)),
            Outcome::Deny { reason, by } => {
                session
                    .emit(Event::ApprovalDecided {
                        call_id: id,
                        decision: Decision::Deny {
                            reason: reason.clone(),
                        },
                        by,
                    })
                    .await?;
                return Ok(Err(denied(&reason)));
            }
            Outcome::Ask(why) => why,
        };
        match ask(session, &call, why).await? {
            Decision::Allow => return Ok(Ok(call)),
            Decision::AllowForSession => {
                for (tool, subject) in crate::permission::grants_for(&call) {
                    session.grant(tool, subject).await;
                }
                return Ok(Ok(call));
            }
            Decision::Deny { reason } => return Ok(Err(denied(&reason))),
            // A rewritten input is a new call as far as the rules go: its
            // risk and subject change, so it goes back through `decide`.
            Decision::Edit { input } => rate(&mut call, tool, input),
        }
    }
}

/// Re-rates `call` for a rewritten `input` (a hook's or the user's edit):
/// risk, subject and segments change together, or the engine would judge
/// the new command by the old one's pieces.
fn rate(call: &mut ToolCall, tool: &dyn Tool, input: Value) {
    call.risk = tool.risk(&input);
    call.subject = tool.subject(&input);
    call.segments = tool.segments(&input);
    call.input = input;
}

/// Emits `ApprovalRequired`, parks until `Submission::Approve` answers it
/// (a cancelled turn answers `Deny`), and emits `ApprovalDecided`.
async fn ask(session: &Session, call: &ToolCall, why: Why) -> Result<Decision, CoreError> {
    let id = call.id;
    // A plugin's call (T33.13) may ask while no turn runs, so the session
    // goes back to the state it was in rather than always `RunningTools`.
    let before = session.inner.lock().await.state;
    let rx = session.await_decision(id).await;
    // Informational like `Stop`: a hook may record that we are waiting, it
    // cannot answer for the user (§1.8).
    let _ = hooks::fire(
        session,
        HookEvent::PermissionRequest,
        serde_json::json!({ "tool_name": call.name, "tool_input": call.input }),
    )
    .await;
    session.advise_approval(call, &why).await?;
    session
        .emit(Event::ApprovalRequired {
            call: call.clone(),
            why,
            // A subagent's prompt is relabelled by `subagent::run_task`
            // on its way to the parent's surface.
            source: Some(Source {
                session: session.id(),
                agent: ORIGIN.try_with(Clone::clone).ok(),
                preset: None,
            }),
        })
        .await?;
    let cancel = session.cancel_token();
    let decision = tokio::select! {
        biased;
        _ = cancel.cancelled() => Decision::Deny { reason: "interrupted".into() },
        d = rx => d.unwrap_or(Decision::Deny { reason: "session closed".into() }),
    };
    session.set_state(before).await;
    session
        .emit(Event::ApprovalDecided {
            call_id: id,
            decision: decision.clone(),
            by: DecidedBy::User,
        })
        .await?;
    Ok(decision)
}

/// What a confined tool reports when the sandbox, not the command, made it
/// fail (`bash` sets `structured.sandbox_denied`).
fn sandbox_denial(output: &ToolOutput) -> Option<String> {
    output
        .structured
        .as_ref()
        .and_then(|s| s.get("sandbox_denied"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[tracing::instrument(
    name = "execute_tool",
    skip_all,
    fields(
        gen_ai.operation.name = "execute_tool",
        gen_ai.tool.name = tool.spec().name,
        gen_ai.tool.call.id = %id,
        gen_ai.tool.call.arguments = tracing::field::Empty,
        gen_ai.tool.call.result = tracing::field::Empty,
        cox.session.id = %session.id,
        cox.tool.risk = ?tool.risk(&input),
        cox.tool.subject = %tool.subject(&input),
        cox.tool.success = tracing::field::Empty,
        cox.tool.duration_ms = tracing::field::Empty,
        cox.tool.output_bytes = tracing::field::Empty,
        cox.archive.id = tracing::field::Empty,
        error.type = tracing::field::Empty,
        otel.status_code = tracing::field::Empty,
    )
)]
async fn run_one(
    session: &Session,
    turn: TurnId,
    id: CallId,
    tool: Arc<dyn Tool>,
    input: Value,
) -> (CallId, ToolResult) {
    let started = Instant::now();
    if let Some(content) = crate::session::captured_json(&input) {
        tracing::Span::current().record("gen_ai.tool.call.arguments", content);
    }
    let (out_tx, mut out_rx) = mpsc::channel::<String>(32);
    // T27.1: `Submission::Background` (or `bash(background: true)`) may
    // detach this call into a task; the card stops streaming once it does.
    let subject = tool.subject(&input);
    let (input, detach) = session.arm_detach(id, &tool.spec().name, input).await;
    let cx = ToolCx {
        roots: session.config.core.workspace_roots.clone(),
        writable_roots: session.writable_roots().to_vec(),
        cwd: session.cwd.clone(),
        sandbox: SandboxPolicy {
            mode: session.config.sandbox.mode,
            network: session.config.sandbox.network,
            writable: session.config.sandbox.writable.clone(),
            readonly_in_workspace: session.config.sandbox.readonly_in_workspace.clone(),
            linux_backend: session.config.sandbox.linux_backend,
        },
        archive: session.archive.clone(),
        cancel: session.cancel_token(),
        output: out_tx,
        session: session.id,
        call: id,
        // T34.3: `None` unless this session is a subagent (`spawn_child`
        // set both), so `ask_user`'s `Question::source` carries the same
        // label `relay_approval` already gives an approval.
        agent: session.agent.clone(),
        preset: session.preset.clone(),
        // T34.6 review: bound per call, to *this* session, never a handle
        // fixed once at tool-construction time — a child's call must reach
        // its own `Relay` impl (`self_task` + emit), not the top-level
        // session's, and this is the one place that distinction is made.
        relay: Some(Arc::new(session.clone()) as Arc<dyn Relay>),
    };
    let pump = session.clone_handle();
    let pump_id = id;
    let pump_detach = detach.clone();
    tokio::spawn(async move {
        loop {
            let delta = tokio::select! {
                biased;
                _ = pump_detach.cancelled() => None,
                delta = out_rx.recv() => delta,
            };
            let Some(delta) = delta else { break };
            let _ = pump
                .emit(Event::ToolCallOutput {
                    call_id: pump_id,
                    delta,
                })
                .await;
        }
    });
    // Only read-only calls are dedup candidates; keep what the key needs.
    let read_key =
        (tool.risk(&input) == Risk::ReadOnly).then(|| (input.clone(), tool.subject(&input)));
    // §1.8 step 8, `on-failure`: the call ran confined without asking, so a
    // sandbox denial is the moment to ask; only an explicit yes reruns it
    // unconfined, anything else keeps the confined result the model can read.
    let retry = (exec_path(session.config.permissions.approval, cx.sandbox.mode)
        == ExecPath::Confined)
        .then(|| input.clone());
    let hook_input = input.clone();
    // T26.1: pre-images before the call can change anything.
    let mut pending = Some(checkpoint::before(session, turn, id, tool.as_ref(), &input).await);
    let running = {
        let tool = tool.clone();
        let call = async move {
            let output = tool.call(input, &cx).await.unwrap_or_else(error_output);
            (output, cx)
        };
        tokio::spawn(call.instrument(tracing::Span::current()))
    };
    let at = crate::tasks::Detachable {
        turn,
        call: id,
        tool: tool.spec().name,
        subject,
    };
    let (mut output, cx) = match session
        .wait_or_detach(at, running, detach, &mut pending)
        .await
    {
        Ok(ran) => ran,
        Err(pointer) => return (id, *pointer),
    };
    if let (Some(input), Some(detail), Some(mut cx)) = (retry, sandbox_denial(&output), cx) {
        let call = ToolCall {
            id,
            name: tool.spec().name,
            risk: tool.risk(&input),
            subject: tool.subject(&input),
            segments: tool.segments(&input),
            input: input.clone(),
        };
        let decision = ask(session, &call, Why::SandboxDenied { detail }).await;
        if let Ok(Decision::Allow | Decision::AllowForSession) = decision {
            cx.sandbox.mode = SandboxMode::DangerFullAccess;
            output = tool.call(input, &cx).await.unwrap_or_else(error_output);
        }
    }
    if let Some(pending) = pending {
        checkpoint::after(session, turn, id, pending).await;
    }
    // §1.8 step vii: informational; the verdict is not applied.
    let event = if output.is_error {
        HookEvent::PostToolUseFailure
    } else {
        HookEvent::PostToolUse
    };
    let _ = hooks::fire(
        session,
        event,
        serde_json::json!({
            "tool_name": tool.spec().name,
            "tool_input": hook_input,
            "tool_response": { "text": output.text, "is_error": output.is_error },
        }),
    )
    .await;
    if crate::session::capture_message_content() {
        tracing::Span::current().record("gen_ai.tool.call.result", output.text.as_str());
    }
    // `tool_search` names what it found in `structured.discovered`; the
    // next request carries those schemas (D6d) and the prefix changes once.
    let found: Vec<String> = output
        .structured
        .as_ref()
        .and_then(|s| s.get("discovered"))
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if !found.is_empty() {
        let found = session.advise_rank(&tool.subject(&hook_input), found).await;
        let added = session.discover(found).await;
        if !added.is_empty() {
            let _ = session
                .emit(Event::Notice {
                    level: Level::Info,
                    text: format!(
                        "discovered tools {}: their schemas join the next request, which \
                         re-caches the prefix once",
                        added.join(", ")
                    ),
                })
                .await;
        }
    }
    // T40.5: an image reaches the model as its own block, so it is archived
    // before anything is sent (the lossless rule) and the text names the row.
    if let Some((media_type, data_b64)) = image::take_structured(&mut output) {
        let line = forward_image(session, id, tool.spec().name, media_type, data_b64).await;
        output.text.push_str(&line);
    }
    let bytes = output.text.len() as u64;
    let archive = session
        .archive
        .put(ArchivePut {
            session: session.id,
            call: id,
            tool: tool.spec().name,
            subject: None,
            bytes: output.text.as_bytes().to_vec(),
        })
        .await;
    let (archive, visible) = match archive {
        Ok(id) => {
            let mut pointer = None;
            if let Some((input, subject)) = read_key.filter(|_| !output.is_error) {
                pointer = session
                    .dedup_observe(
                        &tool.spec().name,
                        &input,
                        &subject,
                        id,
                        output.text.as_bytes(),
                    )
                    .await;
            }
            let visible = pointer.unwrap_or_else(|| {
                crate::truncate::visible(
                    &output.text,
                    id,
                    session.config.context.tool_output_visible_bytes as usize,
                    session.config.context.tool_output_head_lines as usize,
                    session.config.context.tool_output_tail_lines as usize,
                )
            });
            (Some(cox_protocol::ArchiveRef { id, bytes }), visible)
        }
        Err(_) => (None, "tool output could not be archived".into()),
    };
    let result = ToolResult {
        ok: !output.is_error,
        visible,
        archive: archive.clone(),
        bytes,
        duration_ms: started.elapsed().as_millis() as u64,
        diff: output.diff,
        structured: output.structured.map(Box::new),
    };
    // T8.2: the request microcompacts old results to `Pointer`s; the stored
    // history keeps the visible text, so remember the handle here.
    if let Some(arch) = &archive {
        session.remember_archive(id, arch.clone()).await;
        tracing::Span::current().record("cox.archive.id", arch.id.to_string());
    }
    let span = tracing::Span::current();
    span.record("cox.tool.success", result.ok);
    span.record("cox.tool.duration_ms", result.duration_ms);
    span.record("cox.tool.output_bytes", result.bytes);
    span.record("otel.status_code", if result.ok { "OK" } else { "ERROR" });
    if !result.ok {
        span.record("error.type", "tool_error");
    }
    tracing::info!(
        event.name = "cox.tool.completed",
        cox.session.id = %session.id,
        gen_ai.tool.name = tool.spec().name,
        gen_ai.tool.call.id = %id,
        success = result.ok,
        duration_ms = result.duration_ms,
        output_bytes = result.bytes,
        archive.id = archive.as_ref().map(|value| value.id.to_string()).unwrap_or_default(),
        "tool call completed"
    );
    (id, result)
}

/// Checks a tool's image, archives its base64 as its own row and holds it
/// for this round's results message (T40.5); returns the line the tool's
/// visible text gains. Checked here too because an MCP server or a plugin
/// can put anything under `structured["image"]`.
async fn forward_image(
    session: &Session,
    call: CallId,
    tool: String,
    media_type: String,
    data_b64: String,
) -> String {
    let checked = Attachment {
        name: tool.clone(),
        media_type,
        data_b64,
    };
    if let Err(e) = image::validate(&checked) {
        return format!("\n[image dropped: {e}]");
    }
    let Attachment {
        media_type,
        data_b64,
        ..
    } = checked;
    let kib = (data_b64.len() / 4 * 3) as f64 / 1024.0;
    let put = session
        .archive
        .put(ArchivePut {
            session: session.id,
            call,
            tool,
            subject: Some("image".into()),
            bytes: data_b64.as_bytes().to_vec(),
        })
        .await;
    let Ok(archive) = put else {
        return format!("\n[image {media_type} could not be archived; not sent]");
    };
    let line = format!(
        "\n[image {media_type}, {kib:.1} KiB, archived as {archive}; visible to the model in \
         this turn only]"
    );
    let image = Content::Image {
        media_type,
        data_b64,
    };
    session.remember_image(call, image).await;
    line
}

pub(crate) fn error_output(e: ToolError) -> ToolOutput {
    ToolOutput {
        text: e.to_string(),
        is_error: true,
        diff: None,
        structured: None,
    }
}

fn failed_result(msg: &str) -> ToolResult {
    ToolResult {
        ok: false,
        visible: msg.into(),
        archive: None,
        bytes: msg.len() as u64,
        duration_ms: 0,
        diff: None,
        structured: None,
    }
}

pub(crate) fn results_message(results: Vec<(CallId, ToolResult)>) -> Message {
    Message {
        role: Role::User,
        content: results
            .into_iter()
            .map(|(id, result)| Content::ToolResult {
                call_id: id,
                content: result.visible,
                is_error: !result.ok,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use cox_protocol::traits::Provider;
    use cox_protocol::types::ProviderEvent;
    use cox_provider::scripted::Scripted;

    use super::*;
    use crate::MemoryStore;

    /// T39.1: a signature the wire streamed between a call's start and end
    /// is kept under that call's id, and the call itself still commits.
    #[tokio::test]
    async fn consume_provider_keeps_signature_by_call_id() {
        let store = Arc::new(MemoryStore::new());
        let session = Session::new(
            cox_protocol::Config::default(),
            Arc::new(Scripted::from_toml("", "").expect("scenario")),
            vec![],
            store.clone(),
            store,
            PathBuf::from("/tmp/cox-turn-signature"),
        )
        .expect("session");
        let (tx, mut rx) = mpsc::channel(8);
        let signed = CallId::new();
        let unsigned = CallId::new();
        for ev in [
            ProviderEvent::ToolUseStart {
                id: signed,
                name: "read".into(),
            },
            ProviderEvent::ToolUseSignature {
                signature: "sig-1".into(),
            },
            ProviderEvent::ToolUseInputDelta {
                text: r#"{"path":"a.rs"}"#.into(),
            },
            ProviderEvent::ToolUseEnd,
            ProviderEvent::ToolUseStart {
                id: unsigned,
                name: "read".into(),
            },
            ProviderEvent::ToolUseEnd,
        ] {
            tx.send(ev).await.expect("send");
        }
        drop(tx);
        let streamed = consume_provider(&session, &mut rx, ItemId::new())
            .await
            .expect("stream");
        assert_eq!(streamed.calls.len(), 2);
        assert_eq!(
            streamed.signatures,
            HashMap::from([(signed, "sig-1".to_string())])
        );
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    /// Returns a PNG under `structured["image"]`, as `read` does (T40.4).
    struct Shot;

    #[async_trait::async_trait]
    impl Tool for Shot {
        fn spec(&self) -> cox_protocol::types::ToolSpec {
            cox_protocol::types::ToolSpec {
                name: "shot".into(),
                description: "a png".into(),
                input_schema: serde_json::json!({"type": "object"}),
                deferred: false,
                risk: Risk::ReadOnly,
                concurrency: Concurrency::Parallel,
            }
        }
        fn subject(&self, _input: &Value) -> String {
            String::new()
        }
        async fn call(&self, _input: Value, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                text: "image/png, 16 B".into(),
                is_error: false,
                diff: None,
                structured: Some(image::to_structured("image/png", PNG)),
            })
        }
    }

    /// `Scripted` on a wire that takes images. Keeps every request and, for
    /// each image in its last message, whether the archive row the tool
    /// result names already held that image when the request was sent.
    struct Seeing {
        inner: Scripted,
        store: Arc<MemoryStore>,
        seen: std::sync::Mutex<Vec<(cox_protocol::types::Request, Vec<bool>)>>,
    }

    #[async_trait::async_trait]
    impl Provider for Seeing {
        fn id(&self) -> cox_protocol::types::ProviderId {
            self.inner.id()
        }
        fn capabilities(&self) -> cox_protocol::types::Caps {
            self.inner.capabilities()
        }
        fn accepts_images(&self, _model: &str) -> bool {
            true
        }
        async fn stream(
            &self,
            req: cox_protocol::types::Request,
            sink: mpsc::Sender<ProviderEvent>,
            cancel: tokio_util::sync::CancellationToken,
        ) -> Result<Usage, cox_protocol::errors::ProviderError> {
            let archived = archived_images(&self.store, &req);
            self.seen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((req.clone(), archived));
            self.inner.stream(req, sink, cancel).await
        }
        async fn count_tokens(
            &self,
            req: &cox_protocol::types::Request,
        ) -> Result<u32, cox_protocol::errors::ProviderError> {
            self.inner.count_tokens(req).await
        }
    }

    fn archived_images(store: &MemoryStore, req: &cox_protocol::types::Request) -> Vec<bool> {
        use cox_protocol::traits::Store as _;
        let Some(last) = req.messages.last() else {
            return vec![];
        };
        let ids: Vec<cox_protocol::ArchiveId> = last
            .content
            .iter()
            .filter_map(|c| match c {
                Content::ToolResult { content, .. } => content
                    .split("archived as ")
                    .nth(1)?
                    .split(';')
                    .next()?
                    .parse()
                    .ok(),
                _ => None,
            })
            .collect();
        last.content
            .iter()
            .filter_map(|c| match c {
                Content::Image { data_b64, .. } => Some(ids.iter().any(|id| {
                    store
                        .archive_get(id)
                        .is_ok_and(|bytes| bytes == data_b64.as_bytes())
                })),
                _ => None,
            })
            .collect()
    }

    /// One turn: the model calls `shot`, then answers.
    async fn shot_turn() -> (Session, Arc<Seeing>) {
        let store = Arc::new(MemoryStore::new());
        let scenario = "[[turn]]\ntext = \"looking\"\ntool_calls = [{ name = \"shot\", input = {} }]\n\n[[turn]]\ntext = \"seen\"\n";
        let provider = Arc::new(Seeing {
            inner: Scripted::from_toml(scenario, "").expect("scenario"),
            store: store.clone(),
            seen: std::sync::Mutex::default(),
        });
        let cwd = PathBuf::from("/tmp/cox-turn-image");
        let mut config = cox_protocol::Config::default();
        config.core.workspace_roots = vec![cwd.clone()];
        // No title job: it would be one more request to `Seeing`.
        config.session.auto_title = false;
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(Shot)];
        let session = Session::new(config, provider.clone(), tools, store.clone(), store, cwd)
            .expect("session");
        session
            .submit(cox_protocol::types::Submission::UserTurn {
                text: "take a shot".into(),
                attachments: vec![],
                confirm_think: false,
            })
            .await
            .expect("turn");
        (session, provider)
    }

    /// T40.5, the lossless rule: the image's archive row exists before the
    /// provider receives the request that carries the image.
    #[tokio::test]
    async fn tool_image_is_archived_before_it_is_sent() {
        let (_session, provider) = shot_turn().await;
        let seen = provider.seen.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(seen.len(), 2, "the tool round and the answer");
        assert_eq!(seen[1].1, vec![true]);
    }

    /// T40.5: the image joins the results message after every `ToolResult`,
    /// and the result's text names the archive row.
    #[tokio::test]
    async fn tool_image_follows_tool_results() {
        let (session, provider) = shot_turn().await;
        let last = {
            let seen = provider.seen.lock().unwrap_or_else(|e| e.into_inner());
            seen[1].0.messages.last().expect("results message").clone()
        };
        let [
            Content::ToolResult { content, .. },
            Content::Image {
                media_type,
                data_b64,
            },
        ] = &last.content[..]
        else {
            panic!("results then image: {:?}", last.content);
        };
        assert!(content.contains("[image image/png, "), "{content}");
        assert!(content.contains("visible to the model in this turn only"));
        assert_eq!(media_type, "image/png");
        assert_eq!(*data_b64, STANDARD.encode(PNG));
        assert_eq!(session.history().await[2], last);
    }

    /// A91: reasoning streams into its own `Thinking` item, which closes
    /// with its duration before the reply's item starts.
    #[tokio::test]
    async fn streamed_thought_is_its_own_item_closed_before_the_reply() {
        let store = Arc::new(MemoryStore::new());
        let session = Session::new(
            cox_protocol::Config::default(),
            Arc::new(Scripted::from_toml("", "").expect("scenario")),
            vec![],
            store.clone(),
            store,
            PathBuf::from("/tmp/cox-turn-thought"),
        )
        .expect("session");
        let mut events = session.events().expect("events");
        let (tx, mut rx) = mpsc::channel(8);
        for text in ["weigh ", "options"] {
            let ev = ProviderEvent::ThinkingDelta { text: text.into() };
            tx.send(ev).await.expect("send");
        }
        let reply = ProviderEvent::TextDelta { text: "ok".into() };
        tx.send(reply).await.expect("send");
        drop(tx);
        let assistant = ItemId::new();
        consume_provider(&session, &mut rx, assistant)
            .await
            .expect("stream");
        let mut seen = Vec::new();
        while let Ok(ev) = events.try_recv() {
            // Only what the stream caused; a new session may announce itself.
            if !seen.is_empty() || matches!(ev, Event::ItemStarted { .. }) {
                seen.push(ev);
            }
        }
        let Some(Event::ItemStarted {
            item: thought,
            kind: ItemKind::Thinking { .. },
        }) = seen.first().cloned()
        else {
            panic!("thought item first: {seen:?}");
        };
        assert!(matches!(
            &seen[1..],
            [
                Event::ThinkingDelta { item: a, .. },
                Event::ThinkingDelta { item: b, .. },
                Event::ThinkingDone { item: c, .. },
                Event::ItemDone { item: d },
                Event::ItemStarted { item: e, kind: ItemKind::AssistantMessage { .. } },
                Event::TextDelta { item: f, .. },
            ] if [a, b, c, d] == [&thought; 4] && [e, f] == [&assistant; 2]
        ));
    }
}
