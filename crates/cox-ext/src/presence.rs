// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Presence records (plan.md A14): one small JSON file per live session
//! under `COX_HOME/presence/`, so concurrent sessions on one workspace can
//! tell each other what they are doing and which files are mid-edit. The
//! writer is a `Hook` — the seam the surface already installs — so the core
//! still spawns nothing and opens no file. Separate from `hooks`, which
//! runs user commands; this is cox's own bookkeeping.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use cox_protocol::config::HooksConfig;
use cox_protocol::ids::SessionId;
use cox_protocol::traits::Hook;
use cox_protocol::types::{HookEvent, HookOutcome, Presence, PresenceStatus};
use serde_json::{Value, json};

/// Seconds without a heartbeat before a record reads as `Stopped`.
/// ponytail: heartbeats ride on hook events, so a long tool-free stream
/// looks stopped after this; a timer heartbeat is the upgrade if that
/// misleads in practice.
pub const STALE_SECS: u64 = 600;
/// Records silent for this long are leftovers of a killed process; a
/// reader deletes them.
const SWEEP_SECS: u64 = 24 * 3600;
/// Edited paths kept per record, newest last.
const KEEP_TOUCHED: usize = 12;

/// Where the records live.
pub fn dir(home: &Path) -> PathBuf {
    home.join("presence")
}

fn file(home: &Path, session: &SessionId) -> PathBuf {
    dir(home).join(format!("{session}.json"))
}

/// Unix seconds now; 0 if the clock is before 1970.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Writes `record` through a temporary file and a rename, so a concurrent
/// reader never sees half a record.
pub fn write(home: &Path, record: &Presence) -> std::io::Result<()> {
    std::fs::create_dir_all(dir(home))?;
    let target = file(home, &record.session);
    let tmp = target.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(record)?)?;
    std::fs::rename(tmp, target)
}

/// Removes a session's record; a missing file is fine.
pub fn remove(home: &Path, session: &SessionId) {
    let _ = std::fs::remove_file(file(home, session));
}

/// Every other session of `project`, newest heartbeat first. One silent
/// for `STALE_SECS` reads as `Stopped`; one silent for `SWEEP_SECS` is
/// deleted. Records are other processes' output: whatever does not parse
/// is skipped.
pub fn others(home: &Path, project: &Path, me: &SessionId, now: u64) -> Vec<Presence> {
    let Ok(entries) = std::fs::read_dir(dir(home)) else {
        return Vec::new();
    };
    let mut out: Vec<Presence> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|path| {
            let record: Presence = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
            if now.saturating_sub(record.updated) > SWEEP_SECS {
                let _ = std::fs::remove_file(&path);
                return None;
            }
            Some(record)
        })
        .filter(|r| r.session != *me && r.project == project)
        .map(|mut r| {
            if now.saturating_sub(r.updated) > STALE_SECS {
                r.status = PresenceStatus::Stopped;
            }
            r
        })
        .collect();
    out.sort_by_key(|r| std::cmp::Reverse(r.updated));
    out
}

/// What the model reads about the others: a warning and one line each;
/// empty when there are none, so nothing is added to a solo session.
pub fn describe(others: &[Presence], now: u64) -> String {
    if others.is_empty() {
        return String::new();
    }
    let mut text = String::from(
        "Other cox sessions are working in this workspace. Their files may be \
         mid-edit and not compile right now: do not revert, reformat or \"fix\" \
         them, and ask the user before touching a file listed below.\n",
    );
    for r in others {
        let files = if r.touched.is_empty() {
            "no files edited yet".to_string()
        } else {
            format!("editing {}", r.touched.join(", "))
        };
        // T44.2: the path is another process's record and reaches the
        // model, so it goes through the one guard; a newline would let it
        // forge a line of this list.
        let worktree = r.worktree.as_ref().map_or_else(String::new, |w| {
            format!(
                " in worktree {}",
                cox_sanitize::sanitize(&w.display().to_string()).replace('\n', " ")
            )
        });
        text.push_str(&format!(
            "- session {} (pid {}){}: {}, turn {}, {}; last seen {}\n",
            r.session,
            r.pid,
            worktree,
            r.status.name(),
            r.turn,
            files,
            ago(now.saturating_sub(r.updated))
        ));
    }
    text
}

/// T44.2: the other live session of `project` already running in
/// `worktree`, if any — a record silent for `STALE_SECS` does not hold it.
/// Warn-only: the caller still starts (one session per worktree is a
/// convention, not a lock).
pub fn holder(
    home: &Path,
    project: &Path,
    worktree: &Path,
    me: &SessionId,
    now: u64,
) -> Option<Presence> {
    others(home, project, me, now)
        .into_iter()
        .find(|r| r.status != PresenceStatus::Stopped && r.worktree.as_deref() == Some(worktree))
}

fn ago(secs: u64) -> String {
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", secs / 60),
        _ => format!("{}h ago", secs / 3600),
    }
}

/// Adds `context` to a `UserPromptSubmit` verdict without losing a
/// rewritten prompt: the core reads `{prompt?, additional_context?}`.
pub fn with_context(verdict: HookOutcome, context: &str) -> HookOutcome {
    if context.is_empty() {
        return verdict;
    }
    let input = match verdict {
        HookOutcome::Continue => json!({ "additional_context": context }),
        HookOutcome::Modify {
            input: Value::String(prompt),
        } => json!({ "prompt": prompt, "additional_context": context }),
        HookOutcome::Modify {
            input: Value::Object(mut fields),
        } => {
            let joined = match fields.get("additional_context").and_then(Value::as_str) {
                Some(earlier) => format!("{earlier}\n{context}"),
                None => context.to_string(),
            };
            fields.insert("additional_context".into(), Value::String(joined));
            Value::Object(fields)
        }
        other => return other,
    };
    HookOutcome::Modify { input }
}

/// Keeps this session's record current and tells each turn about the
/// others. Wraps the user's `ShellHooks` (if any) so the session still
/// installs one `Hook`.
pub struct PresenceHook {
    home: PathBuf,
    inner: Option<Arc<dyn Hook>>,
    record: Mutex<Presence>,
}

impl PresenceHook {
    pub fn new(
        home: PathBuf,
        session: SessionId,
        cwd: PathBuf,
        project: PathBuf,
        inner: Option<Arc<dyn Hook>>,
    ) -> Self {
        Self {
            home,
            inner,
            record: Mutex::new(Presence {
                session,
                pid: std::process::id(),
                cwd,
                project,
                status: PresenceStatus::Idle,
                turn: 0,
                touched: Vec::new(),
                updated: now_secs(),
                worktree: None,
            }),
        }
    }

    /// Records the worktree the session runs in (`cox --worktree`, T27.3),
    /// so `/agents` can tell a worktree session from one on the main tree.
    pub fn with_worktree(self, worktree: Option<PathBuf>) -> Self {
        {
            let mut record = self.record.lock().unwrap_or_else(|e| e.into_inner());
            record.worktree = worktree;
        }
        self
    }

    fn update(&self, change: impl FnOnce(&mut Presence)) {
        let mut record = self.record.lock().unwrap_or_else(|e| e.into_inner());
        change(&mut record);
        record.updated = now_secs();
        // A full disk or a read-only home must not stop a turn (D14).
        let _ = write(&self.home, &record);
    }

    fn identity(&self) -> (SessionId, PathBuf) {
        let record = self.record.lock().unwrap_or_else(|e| e.into_inner());
        (record.session, record.project.clone())
    }
}

/// Remembers the path an `edit`/`write` call changed, newest last.
fn touch(record: &mut Presence, payload: &Value) {
    let tool = payload
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(tool, "edit" | "write") {
        return;
    }
    let Some(path) = payload.pointer("/tool_input/path").and_then(Value::as_str) else {
        return;
    };
    record.touched.retain(|p| p != path);
    record.touched.push(path.to_string());
    if record.touched.len() > KEEP_TOUCHED {
        record.touched.remove(0);
    }
}

#[async_trait]
impl Hook for PresenceHook {
    /// Whatever the wrapped sources want: presence itself ignores the
    /// observe-only triggers, and a plugin's granted hook is not in
    /// `[hooks]`, so the config check alone would never run it (PL§6).
    fn interested(&self, event: HookEvent, config: &HooksConfig) -> bool {
        self.inner
            .as_ref()
            .is_some_and(|inner| inner.interested(event, config))
    }

    async fn run(&self, event: HookEvent, payload: Value, timeout: Duration) -> HookOutcome {
        match event {
            HookEvent::UserPromptSubmit => self.update(|r| {
                r.status = PresenceStatus::Active;
                r.turn += 1;
            }),
            HookEvent::PreToolUse | HookEvent::PostToolUseFailure => {
                self.update(|r| r.status = PresenceStatus::Active);
            }
            HookEvent::PostToolUse => self.update(|r| {
                r.status = PresenceStatus::Active;
                touch(r, &payload);
            }),
            HookEvent::PermissionRequest => self.update(|r| r.status = PresenceStatus::Waiting),
            HookEvent::Stop => self.update(|r| r.status = PresenceStatus::Idle),
            HookEvent::SessionEnd => remove(&self.home, &self.identity().0),
            _ => {}
        }
        let verdict = match &self.inner {
            Some(inner) => inner.run(event, payload, timeout).await,
            None => HookOutcome::Continue,
        };
        if event != HookEvent::UserPromptSubmit {
            return verdict;
        }
        let (me, project) = self.identity();
        let now = now_secs();
        with_context(
            verdict,
            &describe(&others(&self.home, &project, &me, now), now),
        )
    }
}

impl Drop for PresenceHook {
    /// A quit without `Shutdown` still takes its record with it.
    fn drop(&mut self) {
        let session = self
            .record
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .session;
        remove(&self.home, &session);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(session: SessionId, project: &str, updated: u64) -> Presence {
        Presence {
            session,
            pid: 7,
            cwd: project.into(),
            project: project.into(),
            status: PresenceStatus::Active,
            turn: 3,
            touched: vec!["src/x.rs".into()],
            updated,
            worktree: None,
        }
    }

    fn read(home: &Path, session: &SessionId) -> Presence {
        serde_json::from_slice(&std::fs::read(file(home, session)).unwrap()).unwrap()
    }

    #[test]
    fn presence_others_excludes_me_and_other_projects_and_marks_stale_stopped() {
        let home = tempfile::tempdir().unwrap();
        let now = 100_000;
        let (me, peer, stale, elsewhere) = (
            SessionId::new(),
            SessionId::new(),
            SessionId::new(),
            SessionId::new(),
        );
        write(home.path(), &record(me, "/w", now)).unwrap();
        write(home.path(), &record(peer, "/w", now - 5)).unwrap();
        write(home.path(), &record(stale, "/w", now - STALE_SECS - 1)).unwrap();
        write(home.path(), &record(elsewhere, "/other", now)).unwrap();
        let seen = others(home.path(), Path::new("/w"), &me, now);
        let ids: Vec<SessionId> = seen.iter().map(|r| r.session).collect();
        assert_eq!(ids, [peer, stale]);
        assert_eq!(seen[0].status, PresenceStatus::Active);
        assert_eq!(seen[1].status, PresenceStatus::Stopped);
        let text = describe(&seen, now);
        assert!(
            text.contains(&format!(
                "session {peer} (pid 7): active, turn 3, editing src/x.rs; last seen just now"
            )),
            "{text}"
        );
        assert!(text.contains("stopped"), "{text}");
        assert!(!text.contains(&me.to_string()), "{text}");
        assert_eq!(describe(&[], now), "");
    }

    #[test]
    fn describe_names_other_sessions_worktree() {
        let now = 100_000;
        let peer = SessionId::new();
        let mut r = record(peer, "/w", now);
        r.worktree = Some("/_worktrees/w-t9\u{1b}[31m\nforged".into());
        let text = describe(&[r], now);
        assert!(
            text.contains(&format!(
                "session {peer} (pid 7) in worktree /_worktrees/w-t9 forged: active"
            )),
            "{text}"
        );
        assert!(!text.contains('\u{1b}'), "{text}");
    }

    #[test]
    fn holder_finds_live_session_on_same_worktree() {
        let home = tempfile::tempdir().unwrap();
        let now = 100_000;
        let (me, peer, elsewhere, main) = (
            SessionId::new(),
            SessionId::new(),
            SessionId::new(),
            SessionId::new(),
        );
        let on = |session, worktree: &str| Presence {
            worktree: Some(worktree.into()),
            ..record(session, "/w", now)
        };
        write(home.path(), &on(me, "/wt/a")).unwrap();
        write(home.path(), &on(peer, "/wt/a")).unwrap();
        write(home.path(), &on(elsewhere, "/wt/b")).unwrap();
        write(home.path(), &record(main, "/w", now)).unwrap();
        let found = holder(home.path(), Path::new("/w"), Path::new("/wt/a"), &me, now);
        assert_eq!(found.map(|r| r.session), Some(peer));
        let none = holder(home.path(), Path::new("/w"), Path::new("/wt/c"), &me, now);
        assert!(none.is_none());
    }

    #[test]
    fn holder_ignores_stale_records() {
        let home = tempfile::tempdir().unwrap();
        let now = 100_000;
        let (me, stale) = (SessionId::new(), SessionId::new());
        let held = Presence {
            worktree: Some("/wt/a".into()),
            ..record(stale, "/w", now - STALE_SECS - 1)
        };
        write(home.path(), &held).unwrap();
        let found = holder(home.path(), Path::new("/w"), Path::new("/wt/a"), &me, now);
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn presence_hook_tracks_status_and_files_and_removes_on_session_end() {
        let home = tempfile::tempdir().unwrap();
        let me = SessionId::new();
        let hook = PresenceHook::new(home.path().into(), me, "/w".into(), "/w".into(), None);
        let run = |event, payload| hook.run(event, payload, Duration::from_secs(1));
        assert_eq!(
            run(HookEvent::UserPromptSubmit, json!({ "prompt": "hi" })).await,
            HookOutcome::Continue
        );
        let r = read(home.path(), &me);
        assert_eq!(
            (r.status, r.turn, r.pid),
            (PresenceStatus::Active, 1, std::process::id())
        );
        run(
            HookEvent::PostToolUse,
            json!({ "tool_name": "edit", "tool_input": { "path": "src/a.rs" } }),
        )
        .await;
        run(
            HookEvent::PostToolUse,
            json!({ "tool_name": "bash", "tool_input": { "command": "ls" } }),
        )
        .await;
        run(
            HookEvent::PostToolUse,
            json!({ "tool_name": "write", "tool_input": { "path": "src/b.rs" } }),
        )
        .await;
        run(
            HookEvent::PostToolUse,
            json!({ "tool_name": "edit", "tool_input": { "path": "src/a.rs" } }),
        )
        .await;
        assert_eq!(read(home.path(), &me).touched, ["src/b.rs", "src/a.rs"]);
        run(HookEvent::PermissionRequest, json!({})).await;
        assert_eq!(read(home.path(), &me).status, PresenceStatus::Waiting);
        run(HookEvent::Stop, json!({})).await;
        assert_eq!(read(home.path(), &me).status, PresenceStatus::Idle);
        run(HookEvent::SessionEnd, json!({})).await;
        assert!(!file(home.path(), &me).exists());
    }

    #[tokio::test]
    async fn presence_hook_adds_the_others_as_context_on_prompt() {
        let home = tempfile::tempdir().unwrap();
        let (me, peer) = (SessionId::new(), SessionId::new());
        write(home.path(), &record(peer, "/w", now_secs())).unwrap();
        let hook = PresenceHook::new(home.path().into(), me, "/w".into(), "/w".into(), None);
        let HookOutcome::Modify { input } = hook
            .run(
                HookEvent::UserPromptSubmit,
                json!({ "prompt": "hi" }),
                Duration::from_secs(1),
            )
            .await
        else {
            panic!("others present means Modify");
        };
        let context = input["additional_context"].as_str().unwrap();
        assert!(context.contains(&format!("session {peer}")), "{context}");
        assert!(context.contains("editing src/x.rs"), "{context}");
        assert!(input.get("prompt").is_none());
    }

    #[test]
    fn presence_with_context_keeps_a_rewritten_prompt_and_joins_context() {
        assert_eq!(
            with_context(HookOutcome::Continue, ""),
            HookOutcome::Continue
        );
        assert_eq!(
            with_context(
                HookOutcome::Modify {
                    input: Value::String("new prompt".into())
                },
                "ctx"
            ),
            HookOutcome::Modify {
                input: json!({ "prompt": "new prompt", "additional_context": "ctx" })
            }
        );
        assert_eq!(
            with_context(
                HookOutcome::Modify {
                    input: json!({ "additional_context": "a" })
                },
                "b"
            ),
            HookOutcome::Modify {
                input: json!({ "additional_context": "a\nb" })
            }
        );
        let block = HookOutcome::Block {
            reason: "no".into(),
        };
        assert_eq!(with_context(block.clone(), "ctx"), block);
    }
}
