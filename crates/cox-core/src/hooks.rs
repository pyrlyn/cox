// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Hook call sites (plan.md T7.4/§1.8): the one place the core asks the
//! runner and applies D14 — a hook that fails to run is a warning, never a
//! stopped turn. The runner itself is a `Hook` trait object the surface
//! installs; the core never spawns a process.

use std::time::Duration;

use cox_protocol::config::{HookConfig, HookKind};
use cox_protocol::types::{Event, HookEvent, HookOutcome, Job, Level};
use serde_json::{Value, json};

use crate::session::Session;
use crate::side::Side;

/// Dispatches `event` only when the installed runner is interested in it
/// (T22.3, T33.11). `SessionStart`/`Notification` are observe-only trigger
/// points nothing else listens for, so an unwanted dispatch would be pure
/// noise to a runner. The runner answers rather than `[hooks]` because a
/// plugin's hooks are granted, not configured (PL§6).
pub(crate) async fn fire_configured(
    session: &Session,
    event: HookEvent,
    extra: Value,
) -> HookOutcome {
    let wanted = session
        .hook()
        .is_some_and(|hook| hook.interested(event, &session.config.hooks));
    if !wanted {
        return HookOutcome::Continue;
    }
    // `Box::pin`: `fire` can emit a `Notice` back through `Session::emit`,
    // which may dispatch here again — the cycle needs one indirect future.
    Box::pin(fire(session, event, extra)).await
}

/// What a `Notification` carries per trigger (T22.3): `kind`, `message`,
/// `title`, on top of `fire`'s common fields. Its stdout is observe-only
/// and never read.
pub(crate) fn notification_payload(event: &Event) -> Option<Value> {
    match event {
        Event::ApprovalRequired { call, .. } => Some(json!({
            "kind": "approval_required",
            "title": "Approval required",
            "message": format!("{}: {}", call.name, call.subject),
        })),
        Event::TurnDone { stop, .. } => Some(json!({
            "kind": "turn_done",
            "title": "Turn done",
            "message": crate::session::stop_reason_name(stop),
        })),
        Event::ToolCallRequested { call } if call.name == "ask_user" => Some(json!({
            "kind": "ask_user",
            "title": "Question",
            "message": call.input.pointer("/question").and_then(Value::as_str).unwrap_or_default(),
        })),
        _ => None,
    }
}

/// Runs `event` with the common payload fields plus `extra` merged in.
pub(crate) async fn fire(session: &Session, event: HookEvent, extra: Value) -> HookOutcome {
    let config = &session.config.hooks;
    // `--no-hooks` decides what the surface installs (T16.2: presence stays
    // on, shell hooks come off); an installed hook always runs.
    let Some(runner) = session.hook() else {
        return HookOutcome::Continue;
    };
    let mut payload = json!({
        "session_id": session.id.to_string(),
        "cwd": session.cwd,
        "hook_event_name": event.name(),
    });
    if let (Some(into), Some(from)) = (payload.as_object_mut(), extra.as_object()) {
        into.extend(from.clone());
    }
    let timeout = Duration::from_secs(u64::from(config.timeout_s));
    // Prompt hooks are judged here: the core already holds the provider,
    // so the call opens no socket. They run before the shell/http runner;
    // a block stops the chain, and a failure follows D14 like a command hook.
    if let Some(stop) = judge_prompts(session, event, &payload).await {
        return stop;
    }
    settle(session, event, runner.run(event, payload, timeout).await).await
}

/// `Failed` becomes a warning and `Continue` when the session fails open,
/// and a block otherwise (D14). A `Block` passes through unchanged.
async fn settle(session: &Session, event: HookEvent, outcome: HookOutcome) -> HookOutcome {
    let fail_open = session.config.hooks.fail_open;
    match outcome {
        HookOutcome::Failed { error } if fail_open => {
            let _ = session
                .emit(Event::Notice {
                    level: Level::Warn,
                    text: format!("hook {} skipped: {error}", event.name()),
                })
                .await;
            HookOutcome::Continue
        }
        HookOutcome::Failed { error } => HookOutcome::Block { reason: error },
        outcome => outcome,
    }
}

/// Walks the prompt hooks configured for `event`. `Some` stops the chain
/// (a judge said `ok: false`, or a failure the session does not fail open).
/// `None` means every prompt allowed the call, including those skipped
/// after a warning.
async fn judge_prompts(
    session: &Session,
    event: HookEvent,
    payload: &Value,
) -> Option<HookOutcome> {
    let hooks: Vec<HookConfig> = session
        .config
        .hooks
        .events
        .get(event.name())
        .cloned()
        .unwrap_or_default();
    let tool = payload
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    for hook in hooks
        .into_iter()
        .filter(|hook| hook.kind == HookKind::Prompt)
    {
        match hook_matches(hook.matcher.as_deref(), tool) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) => {
                let failed = HookOutcome::Failed {
                    error: format!(
                        "{}: invalid matcher regex {:?}: {error}",
                        hook.command, hook.matcher
                    ),
                };
                if let HookOutcome::Block { reason } = settle(session, event, failed).await {
                    return Some(HookOutcome::Block { reason });
                }
                continue;
            }
        }
        let verdict = prompt_verdict(ask_judge(session, &hook.command, payload).await.as_deref());
        if let HookOutcome::Block { reason } = settle(session, event, verdict).await {
            return Some(HookOutcome::Block { reason });
        }
    }
    None
}

/// The cheap-tier judge sees the hook's prompt and the hook input, and no
/// tools (`side_call` sends an empty tool list). `None` is a failed call.
async fn ask_judge(session: &Session, prompt: &str, payload: &Value) -> Option<String> {
    let system = format!(
        "{prompt}\n\nAnswer with one JSON object {{\"ok\": bool, \"reason\": string}} and nothing else."
    );
    let text = payload.to_string();
    let turn = session.inner.lock().await.turn_seq;
    session
        .side_call(Side {
            job: Job::Hook,
            turn,
            system: &system,
            text: &text,
            max_tokens: 256,
            max_chars: 4_000,
        })
        .await
}

/// Anything but a JSON object with `ok: false` allows the call. A missing
/// or empty `reason` still blocks. Unparseable text and a missing answer
/// are `Failed`, which [`settle`] fails open.
fn prompt_verdict(answer: Option<&str>) -> HookOutcome {
    let Some(answer) = answer.map(str::trim).filter(|s| !s.is_empty()) else {
        return HookOutcome::Failed {
            error: "prompt hook failed".into(),
        };
    };
    match serde_json::from_str::<Value>(answer) {
        Ok(value) if value.get("ok").and_then(Value::as_bool) == Some(false) => {
            let reason = value
                .get("reason")
                .and_then(Value::as_str)
                .filter(|reason| !reason.is_empty())
                .unwrap_or("blocked by hook");
            HookOutcome::Block {
                reason: reason.to_string(),
            }
        }
        Ok(_) => HookOutcome::Continue,
        Err(_) => HookOutcome::Failed {
            error: "unparseable prompt hook answer".into(),
        },
    }
}

/// Same matcher rule as the shell runner: absent, empty or `*` matches
/// every tool; a pattern with no regex metacharacters is an exact name;
/// anything else is an unanchored regex. Kept here because the judge
/// cannot call `cox-ext`.
fn hook_matches(matcher: Option<&str>, tool: &str) -> Result<bool, regex::Error> {
    match matcher.map(str::trim) {
        None | Some("") | Some("*") => Ok(true),
        Some(pattern) if !pattern.contains(|c: char| ".$^*+?()[]{}|\\".contains(c)) => {
            Ok(pattern == tool)
        }
        Some(pattern) => regex::Regex::new(pattern).map(|re| re.is_match(tool)),
    }
}

/// What a `UserPromptSubmit` `Modify` means (T16.2): a string replaces the
/// prompt; an object may carry `prompt` and/or `additional_context`. The
/// context rides to the model as a second text block after the last cache
/// breakpoint and is never shown as the user's words.
pub(crate) fn prompt_rewrite(text: String, input: Value) -> (String, Option<String>) {
    match input {
        Value::String(s) => (s, None),
        Value::Object(o) => {
            let prompt = o
                .get("prompt")
                .and_then(Value::as_str)
                .map_or(text, str::to_owned);
            let context = o
                .get("additional_context")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty())
                .map(str::to_owned);
            (prompt, context)
        }
        _ => (text, None),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use cox_protocol::ProviderError;
    use cox_protocol::traits::{Hook, Provider, Store};
    use cox_protocol::types::{Content, ProviderEvent, ProviderId, Request, Tier};
    use cox_provider::scripted::Scripted;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::session::{MemoryStore, Session};

    #[test]
    fn prompt_rewrite_accepts_a_string_or_a_prompt_and_context_object() {
        let t = || "hi".to_owned();
        assert_eq!(prompt_rewrite(t(), json!("new")), ("new".into(), None));
        assert_eq!(
            prompt_rewrite(t(), json!({ "additional_context": "peer" })),
            ("hi".into(), Some("peer".into()))
        );
        assert_eq!(
            prompt_rewrite(t(), json!({ "prompt": "new", "additional_context": "" })),
            ("new".into(), None)
        );
        assert_eq!(prompt_rewrite(t(), json!(7)), ("hi".into(), None));
    }

    /// Records the judge's request, then answers with the scripted turn.
    struct Seen {
        script: Scripted,
        requests: Mutex<Vec<Request>>,
    }

    #[async_trait]
    impl Provider for Seen {
        fn id(&self) -> ProviderId {
            self.script.id()
        }
        fn capabilities(&self) -> cox_protocol::types::Caps {
            self.script.capabilities()
        }
        async fn stream(
            &self,
            req: Request,
            sink: mpsc::Sender<ProviderEvent>,
            cancel: CancellationToken,
        ) -> Result<cox_protocol::types::Usage, ProviderError> {
            self.requests
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(req.clone());
            self.script.stream(req, sink, cancel).await
        }
        async fn count_tokens(&self, req: &Request) -> Result<u32, ProviderError> {
            self.script.count_tokens(req).await
        }
    }

    struct Allow;

    #[async_trait]
    impl Hook for Allow {
        async fn run(&self, _: HookEvent, _: Value, _: Duration) -> HookOutcome {
            HookOutcome::Continue
        }
    }

    fn open_prompt(answer: &str) -> (Session, Arc<MemoryStore>, Arc<Seen>) {
        let mut config = cox_protocol::Config::default();
        config.hooks.events.insert(
            "PreToolUse".into(),
            vec![HookConfig {
                command: "is this safe?".into(),
                kind: HookKind::Prompt,
                ..HookConfig::default()
            }],
        );
        let store = Arc::new(MemoryStore::new());
        let script = Scripted::from_toml(&format!("[[turn]]\ntext = '''{answer}'''\n"), "")
            .expect("scenario");
        let seen = Arc::new(Seen {
            script,
            requests: Mutex::new(Vec::new()),
        });
        let session = Session::new(
            config,
            seen.clone(),
            vec![],
            store.clone(),
            store.clone(),
            PathBuf::from("/tmp/cox-hook"),
        )
        .expect("session");
        session.set_hook(Arc::new(Allow));
        (session, store, seen)
    }

    fn judge_text(req: &Request) -> String {
        let mut text = req
            .system
            .iter()
            .map(|block| block.text.clone())
            .collect::<Vec<_>>()
            .join("\n");
        for message in &req.messages {
            for block in &message.content {
                if let Content::Text { text: line } = block {
                    text.push('\n');
                    text.push_str(line);
                }
            }
        }
        text
    }

    #[tokio::test]
    async fn prompt_hook_blocks_when_judge_says_not_ok() {
        let (session, store, seen) = open_prompt(r#"{"ok": false, "reason": "no"}"#);
        let out = fire(
            &session,
            HookEvent::PreToolUse,
            json!({ "tool_name": "bash", "tool_input": { "command": "rm -rf /" } }),
        )
        .await;
        assert_eq!(
            out,
            HookOutcome::Block {
                reason: "no".into()
            }
        );
        let requests = seen.requests.lock().unwrap_or_else(|e| e.into_inner());
        let req = requests.first().expect("the judge was called");
        assert!(req.tools.is_empty(), "the judge has no tools");
        let text = judge_text(req);
        assert!(text.contains("is this safe?"));
        assert!(text.contains("rm -rf /"));
        assert!(
            store
                .usage_rows()
                .iter()
                .any(|row| row.job == Job::Hook && row.tier == Tier::Cheap),
            "the side call is a usage row on the cheap tier"
        );
    }

    #[tokio::test]
    async fn prompt_hook_unparseable_answer_fails_open() {
        let (session, store, _) = open_prompt("not json");
        let out = fire(
            &session,
            HookEvent::PreToolUse,
            json!({ "tool_name": "bash" }),
        )
        .await;
        assert_eq!(out, HookOutcome::Continue);
        let events = store.rollout_read(&session.id).expect("rollout");
        assert!(events.iter().any(|event| matches!(
            event,
            Event::Notice { level: Level::Warn, text } if text.contains("unparseable")
        )));
    }
}
