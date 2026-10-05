// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The hook runner (plan.md T7.4, D4/D14): runs `[[hooks.<Event>]]`
//! commands over Claude Code's JSON protocol — payload on stdin, verdict on
//! stdout, exit 2 = block. It lives here, not in `cox-core`, because it
//! spawns processes; the core only sees a `HookOutcome` and fails open.
//! Also the one chaining rule ([`chain`]) and [`HookChain`], which puts the
//! shell hooks and plugin hooks behind one `Hook` (PL§6, T33.11).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cox_protocol::config::{HookConfig, HooksConfig};
use cox_protocol::traits::Hook;
use cox_protocol::types::{HookEvent, HookOutcome};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

pub struct ShellHooks {
    events: HashMap<String, Vec<HookConfig>>,
    cwd: PathBuf,
    /// The program that runs each command with `-c`: `sh` from `PATH`
    /// unless the session passes the host's shell (T57.2: Git Bash or
    /// PowerShell on Windows, the same program a `!` line runs in).
    shell: PathBuf,
}

impl ShellHooks {
    pub fn new(config: &HooksConfig, cwd: PathBuf) -> Self {
        Self {
            events: config.events.clone(),
            cwd,
            shell: PathBuf::from("sh"),
        }
    }

    pub fn with_shell(mut self, shell: PathBuf) -> Self {
        self.shell = shell;
        self
    }
}

#[async_trait]
impl Hook for ShellHooks {
    /// Hooks for `event` run in config order under [`chain`]'s rule.
    async fn run(&self, event: HookEvent, payload: Value, timeout: Duration) -> HookOutcome {
        let Some(hooks) = self.events.get(event.name()) else {
            return HookOutcome::Continue;
        };
        let tool = payload
            .get("tool_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        chain(hooks, payload, |hook, payload| {
            let (tool, cwd, shell) = (&tool, &self.cwd, &self.shell);
            async move {
                match matches(hook.matcher.as_deref(), tool) {
                    Ok(true) => {}
                    Ok(false) => return HookOutcome::Continue,
                    // D14 fail open: a broken matcher is this hook's failure —
                    // skipped with the core's `Notice(Warn)` naming it.
                    Err(e) => {
                        return HookOutcome::Failed {
                            error: format!(
                                "{}: invalid matcher regex {:?}: {e}",
                                hook.command, hook.matcher
                            ),
                        };
                    }
                }
                let limit = hook
                    .timeout_s
                    .map_or(timeout, |s| Duration::from_secs(u64::from(s)));
                run_one(shell, &hook.command, &payload, limit, cwd).await
            }
        })
        .await
    }
}

/// The one chaining rule for every hook source (PL§6): steps run in order,
/// the first `Block` or `Failed` ends the chain, and a `Modify` feeds its
/// input to the steps after it as `tool_input` and is the chain's verdict
/// unless a later step modifies again. Shared by a source's own hooks
/// (`ShellHooks`) and by [`HookChain`] across sources, so the two levels
/// cannot drift apart.
pub async fn chain<T, F, Fut>(
    steps: impl IntoIterator<Item = T>,
    mut payload: Value,
    mut step: F,
) -> HookOutcome
where
    F: FnMut(T, Value) -> Fut,
    Fut: Future<Output = HookOutcome>,
{
    let mut outcome = HookOutcome::Continue;
    for item in steps {
        match step(item, payload.clone()).await {
            HookOutcome::Continue => {}
            HookOutcome::Modify { input } => {
                if let Some(fields) = payload.as_object_mut() {
                    fields.insert("tool_input".into(), input.clone());
                }
                outcome = HookOutcome::Modify { input };
            }
            stop => return stop,
        }
    }
    outcome
}

/// Every hook source of a session as one `Hook` (PL§6): the user's shell
/// hooks first, so their own rules win, then plugin hooks in plugin-id
/// order. `PresenceHook` wraps the chain, as it wrapped `ShellHooks`.
pub struct HookChain(Vec<Arc<dyn Hook>>);

impl HookChain {
    /// `shell` first, then `plugins` sorted by id, whatever order the
    /// caller loaded them in.
    pub fn new(shell: Option<Arc<dyn Hook>>, mut plugins: Vec<(String, Arc<dyn Hook>)>) -> Self {
        plugins.sort_by(|a, b| a.0.cmp(&b.0));
        Self(
            shell
                .into_iter()
                .chain(plugins.into_iter().map(|(_, hook)| hook))
                .collect(),
        )
    }
}

#[async_trait]
impl Hook for HookChain {
    fn interested(&self, event: HookEvent, config: &HooksConfig) -> bool {
        self.0.iter().any(|hook| hook.interested(event, config))
    }

    /// Each source filters its own events, so every source is asked; the
    /// core applies the deadline and fail-open to the chain's verdict.
    async fn run(&self, event: HookEvent, payload: Value, timeout: Duration) -> HookOutcome {
        chain(&self.0, payload, |hook, payload| {
            hook.run(event, payload, timeout)
        })
        .await
    }
}

/// Claude's matcher (T22.3): absent, empty or `*` matches everything; a
/// pattern without regex metacharacters is an exact tool name; anything
/// else compiles with `regex` and is searched unanchored against the tool
/// name, like Claude Code's `.test`. An invalid regex is `Err`, which the
/// runner turns into a skipped hook and a warning — never a config fatal.
fn matches(matcher: Option<&str>, tool: &str) -> Result<bool, regex::Error> {
    match matcher.map(str::trim) {
        None | Some("") | Some("*") => Ok(true),
        Some(m) if !m.contains(|c: char| ".$^*+?()[]{}|\\".contains(c)) => Ok(m == tool),
        Some(m) => regex::Regex::new(m).map(|re| re.is_match(tool)),
    }
}

async fn run_one(
    shell: &Path,
    command: &str,
    payload: &Value,
    limit: Duration,
    cwd: &PathBuf,
) -> HookOutcome {
    let failed = |error: String| HookOutcome::Failed { error };
    let mut child = match Command::new(shell)
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(e) => return failed(format!("spawn failed: {e}")),
    };
    let pid = child.id();
    if let Some(mut stdin) = child.stdin.take() {
        // A hook that never reads stdin is fine; a closed pipe is not an error.
        let _ = stdin.write_all(payload.to_string().as_bytes()).await;
    }
    let output = match tokio::time::timeout(limit, child.wait_with_output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return failed(format!("wait failed: {e}")),
        Err(_) => {
            // `kill_on_drop` took the shell; the group takes its children.
            if let Some(pid) = pid {
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(pid as i32),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
            return failed(format!("timed out after {}s", limit.as_secs()));
        }
    };
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    match output.status.code() {
        Some(0) => verdict(&String::from_utf8_lossy(&output.stdout)),
        Some(2) => HookOutcome::Block {
            reason: if stderr.is_empty() {
                "blocked by hook".into()
            } else {
                stderr
            },
        },
        Some(code) => failed(format!("exit {code}: {stderr}")),
        None => failed(format!("killed by signal: {stderr}")),
    }
}

/// Exit 0 stdout: JSON with `continue:false`, `decision:"block"`,
/// `hookSpecificOutput.permissionDecision:"deny"`, `updatedInput` or
/// `additionalContext`; anything else (including plain text) means continue.
fn verdict(stdout: &str) -> HookOutcome {
    let Ok(v) = serde_json::from_str::<Value>(stdout.trim()) else {
        return HookOutcome::Continue;
    };
    let reason = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| v.pointer(k).and_then(Value::as_str))
            .unwrap_or("blocked by hook")
            .to_string()
    };
    let specific = v.get("hookSpecificOutput").cloned().unwrap_or(Value::Null);
    if v.get("continue") == Some(&Value::Bool(false)) {
        return HookOutcome::Block {
            reason: reason(&["/stopReason", "/reason"]),
        };
    }
    if v.get("decision").and_then(Value::as_str) == Some("block")
        || specific.get("permissionDecision").and_then(Value::as_str) == Some("deny")
    {
        return HookOutcome::Block {
            reason: reason(&["/reason", "/hookSpecificOutput/permissionDecisionReason"]),
        };
    }
    let verdict = match specific
        .get("updatedInput")
        .or_else(|| v.get("updatedInput"))
    {
        Some(input) => HookOutcome::Modify {
            input: input.clone(),
        },
        None => HookOutcome::Continue,
    };
    let context = specific
        .get("additionalContext")
        .or_else(|| v.get("additionalContext"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    crate::presence::with_context(verdict, context)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hooks_matcher_is_exact_or_prefix_glob() {
        assert!(matches(None, "bash").unwrap());
        assert!(matches(Some("*"), "bash").unwrap());
        assert!(matches(Some("bash|edit"), "edit").unwrap());
        assert!(matches(Some("mcp__*"), "mcp__x__y").unwrap());
        assert!(!matches(Some("bash"), "bashful").unwrap());
    }

    #[test]
    fn matcher_regex_matches_bash_or_edit() {
        assert!(matches(Some("bash|edit"), "bash").unwrap());
        assert!(matches(Some("bash|edit"), "edit").unwrap());
        assert!(!matches(Some("bash|edit"), "read").unwrap());
        assert!(matches(Some("^mcp__.*"), "mcp__srv__tool").unwrap());
    }

    #[tokio::test]
    async fn broken_matcher_regex_names_the_hook_and_skips_it() {
        let mut events = HashMap::new();
        events.insert(
            "PreToolUse".to_string(),
            vec![HookConfig {
                matcher: Some("(".into()),
                command: "echo never".into(),
                timeout_s: None,
            }],
        );
        let config = HooksConfig {
            events,
            ..HooksConfig::default()
        };
        let hooks = ShellHooks::new(&config, PathBuf::from("."));
        let out = hooks
            .run(
                HookEvent::PreToolUse,
                serde_json::json!({ "tool_name": "bash" }),
                Duration::from_secs(5),
            )
            .await;
        assert!(
            matches!(out, HookOutcome::Failed { ref error }
                if error.contains("echo never") && error.contains("matcher")),
            "{out:?}"
        );
    }

    /// A plugin-like source: granted `PreToolUse` only, answers `verdict`
    /// and records the `tool_input` it was handed.
    struct Fake {
        verdict: HookOutcome,
        seen: std::sync::Mutex<Vec<Value>>,
    }

    fn fake(verdict: HookOutcome) -> Arc<Fake> {
        Arc::new(Fake {
            verdict,
            seen: std::sync::Mutex::new(Vec::new()),
        })
    }

    #[async_trait]
    impl Hook for Fake {
        fn interested(&self, event: HookEvent, _config: &HooksConfig) -> bool {
            event == HookEvent::PreToolUse
        }

        async fn run(&self, _event: HookEvent, payload: Value, _timeout: Duration) -> HookOutcome {
            self.seen
                .lock()
                .unwrap()
                .push(payload["tool_input"].clone());
            self.verdict.clone()
        }
    }

    fn shell(command: &str) -> (HooksConfig, Arc<dyn Hook>) {
        let mut events = HashMap::new();
        events.insert(
            "PreToolUse".to_string(),
            vec![HookConfig {
                matcher: None,
                command: command.into(),
                timeout_s: None,
            }],
        );
        let config = HooksConfig {
            events,
            ..HooksConfig::default()
        };
        let hooks = Arc::new(ShellHooks::new(&config, PathBuf::from(".")));
        (config, hooks)
    }

    fn pre_tool_use() -> Value {
        serde_json::json!({ "tool_name": "bash", "tool_input": { "command": "rm -rf /" } })
    }

    #[tokio::test]
    async fn shell_block_wins_over_plugin() {
        let (config, shell) = shell("echo mine >&2; exit 2");
        let plugin = fake(HookOutcome::Modify {
            input: serde_json::json!({ "command": "ls" }),
        });
        let chain = HookChain::new(Some(shell), vec![("p".into(), plugin.clone() as _)]);
        assert!(chain.interested(HookEvent::PreToolUse, &config));
        let out = chain
            .run(
                HookEvent::PreToolUse,
                pre_tool_use(),
                Duration::from_secs(5),
            )
            .await;
        assert_eq!(
            out,
            HookOutcome::Block {
                reason: "mine".into()
            }
        );
        assert!(
            plugin.seen.lock().unwrap().is_empty(),
            "the plugin never ran"
        );
    }

    #[tokio::test]
    async fn plugin_modify_chains_into_next_hook() {
        let (_, shell) = shell("true");
        let first = fake(HookOutcome::Modify {
            input: serde_json::json!({ "command": "ls" }),
        });
        let second = fake(HookOutcome::Continue);
        // Loaded out of order; the chain runs plugins by id.
        let chain = HookChain::new(
            Some(shell),
            vec![
                ("b".into(), second.clone() as _),
                ("a".into(), first.clone() as _),
            ],
        );
        // No `[hooks]` entry is needed for a plugin's event to be wanted.
        assert!(chain.interested(HookEvent::PreToolUse, &HooksConfig::default()));
        assert!(!chain.interested(HookEvent::Stop, &HooksConfig::default()));
        let out = chain
            .run(
                HookEvent::PreToolUse,
                pre_tool_use(),
                Duration::from_secs(5),
            )
            .await;
        assert_eq!(
            *first.seen.lock().unwrap(),
            [serde_json::json!({ "command": "rm -rf /" })]
        );
        assert_eq!(
            *second.seen.lock().unwrap(),
            [serde_json::json!({ "command": "ls" })]
        );
        assert_eq!(
            out,
            HookOutcome::Modify {
                input: serde_json::json!({ "command": "ls" })
            }
        );
    }

    #[test]
    fn hooks_verdict_reads_claude_shapes() {
        assert_eq!(verdict("all good"), HookOutcome::Continue);
        assert_eq!(
            verdict(r#"{"decision":"block","reason":"no"}"#),
            HookOutcome::Block {
                reason: "no".into()
            }
        );
        assert_eq!(
            verdict(
                r#"{"hookSpecificOutput":{"permissionDecision":"deny","permissionDecisionReason":"nope"}}"#
            ),
            HookOutcome::Block {
                reason: "nope".into()
            }
        );
        assert_eq!(
            verdict(r#"{"continue":false,"stopReason":"halt"}"#),
            HookOutcome::Block {
                reason: "halt".into()
            }
        );
        assert_eq!(
            verdict(r#"{"hookSpecificOutput":{"updatedInput":{"command":"ls"}}}"#),
            HookOutcome::Modify {
                input: serde_json::json!({"command":"ls"})
            }
        );
        assert_eq!(
            verdict(r#"{"hookSpecificOutput":{"additionalContext":"remember x"}}"#),
            HookOutcome::Modify {
                input: serde_json::json!({"additional_context":"remember x"})
            }
        );
    }
}
