// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox --plain` (T29.1): the fifth consumer of the core's `Event` stream,
//! for screen readers and terminals that cannot redraw. Every event becomes
//! whole labelled lines appended to stdout — no cursor movement, no line is
//! ever rewritten — so scrollback is the full transcript and a screen reader
//! reads each line once. Separate from `session.rs` because it shares none
//! of the TUI's state, only `session::open`, `cox_tui::commands::parse` and
//! `cox_sanitize::sanitize` (T32.1: no longer routed through `cox-tui`).

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::time::Duration;

use cox_core::Session;
use cox_protocol::CoreError;
use cox_protocol::Event;
use cox_protocol::ids::{CallId, ItemId};
use cox_protocol::types::{Decision, ItemKind, Level, StopReason, Submission, Tier, ToolResult};
use cox_sanitize::sanitize;
use cox_tui::commands::{self, Action};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::cli::Cli;
use crate::{config_load, session};

/// Lines of a tool result shown from each end; the rest is counted.
const HEAD_TAIL: usize = 3;
const USER: &str = "you: ";
const APPROVE: &str = "approve? [1] allow [2] session [3] deny ";

/// A pending question the next stdin line answers, oldest first.
enum Ask {
    Approval(CallId),
    Question {
        call_id: CallId,
        text: String,
        options: Vec<String>,
        /// Who asks when it is not the model (T47.3): a subagent's name or
        /// `mcp:<server>` for an MCP elicitation.
        from: Option<String>,
    },
}

/// What a typed line asked for.
enum Next {
    Prompt,
    Busy(JoinHandle<Result<(), CoreError>>),
    Quit,
}

/// Runs the plain surface until `/quit`, a second idle `Ctrl+C` or EOF.
pub fn run(cli: &Cli, cwd: &Path) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    let home = cli.home.clone().unwrap_or_else(config_load::cox_home);
    let resume = session::resume_from_flags(cli, &home, cwd)?;
    let resumed = resume.as_ref().map(|(id, _)| *id);
    let (session, loaded) = rt.block_on(session::open(
        cli,
        cwd,
        None,
        // T22.1's surface: `ask_user` questions come here instead of `--answer`.
        true,
        |_| {},
        resume,
        true,
        None,
    ))?;
    let mut plain = Plain {
        session: session.clone(),
        full_thinking: loaded.config.tui.show_thinking == "full",
        texts: HashMap::new(),
        asks: VecDeque::new(),
        prompt: None,
        armed: false,
        turn_usd: 0.0,
        turn_tokens: [0, 0],
        session_usd: 0.0,
        tui: {
            let mut tui = cox_tui::state::State::new(
                loaded.config.permissions.mode,
                loaded.config.sandbox.mode,
            );
            tui.status.budget_cap_usd = loaded.config.budget.session_usd;
            tui.status.budget_warn_at = loaded.config.budget.warn_at;
            tui
        },
    };
    if let Some(id) = resumed {
        plain.line("notice", &format!("resumed session {id}"))?;
    }
    // Assistive tech may still be announcing the terminal when cox starts.
    let quiet = std::env::var("COX_AX_STARTUP_QUIET_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map(Duration::from_millis);
    rt.block_on(plain.drive(cli.prompt.clone(), quiet))?;
    rt.block_on(session.submit(Submission::Shutdown))?;
    Ok(())
}

struct Plain {
    session: Session,
    full_thinking: bool,
    /// Assistant and thinking text, printed whole at `ItemDone` so a table
    /// can be flattened and a screen reader hears one line per line.
    texts: HashMap<ItemId, (&'static str, String)>,
    asks: VecDeque<Ask>,
    /// The prompt now on screen, repeated after any line printed under it.
    prompt: Option<&'static str>,
    /// An idle `Ctrl+C` arms; a second one quits (the TUI's rule).
    armed: bool,
    turn_usd: f64,
    /// This turn's context (input + cache) and output tokens.
    turn_tokens: [u32; 2],
    session_usd: f64,
    /// The TUI status row's source of truth (T28.1): `Usage` fills the
    /// context and cache share, `TurnStarted` the model, and `TurnDone`
    /// prints `status::plain_text` — the same segments as the TUI row.
    tui: cox_tui::state::State,
}

impl Plain {
    async fn drive(
        &mut self,
        first: Option<String>,
        quiet: Option<Duration>,
    ) -> anyhow::Result<()> {
        let mut events = self
            .session
            .events()
            .ok_or_else(|| anyhow::anyhow!("session events already taken"))?;
        let mut lines = spawn_stdin();
        let mut sigint = spawn_sigint();
        // A terminal echoes what was typed after the prompt; a pipe does not.
        let echo = !std::io::stdin().is_terminal();
        let mut busy: Option<JoinHandle<Result<(), CoreError>>> = None;
        let mut eof = false;
        if let Some(quiet) = quiet {
            tokio::time::sleep(quiet).await;
        }
        match first.filter(|p| !p.trim().is_empty()) {
            Some(text) => {
                self.line("you", &text)?;
                busy = Some(self.spawn(user_turn(text)));
            }
            None => self.show_prompt(USER)?,
        }
        loop {
            let reading = !eof && (busy.is_none() || !self.asks.is_empty());
            tokio::select! {
                Some(ev) = events.recv() => self.event(ev)?,
                done = join(&mut busy), if busy.is_some() => {
                    busy = None;
                    // `submit` returns after `TurnDone` is sent; print what is
                    // still queued before the next prompt.
                    while let Ok(ev) = events.try_recv() {
                        self.event(ev)?;
                    }
                    if let Ok(Err(e)) = done {
                        self.line("error", &sanitize(&e.to_string()))?;
                    }
                    self.asks.clear();
                    if eof {
                        break;
                    }
                    self.show_prompt(USER)?;
                }
                line = lines.recv(), if reading => {
                    let Some(line) = line else {
                        eof = true;
                        self.cancel_asks().await?;
                        if busy.is_none() {
                            break;
                        }
                        continue;
                    };
                    if echo && self.prompt.is_some() {
                        println!("{line}");
                    }
                    self.prompt = None;
                    self.armed = false;
                    if !self.asks.is_empty() {
                        self.answer(&line).await?;
                        continue;
                    }
                    match self.input(&line)? {
                        Next::Prompt => self.show_prompt(USER)?,
                        Next::Busy(handle) => busy = Some(handle),
                        Next::Quit => break,
                    }
                }
                Some(()) = sigint.recv() => {
                    if busy.is_some() {
                        self.session.interrupt();
                        self.cancel_asks().await?;
                    } else if self.armed {
                        break;
                    } else {
                        self.armed = true;
                        self.line("notice", "press Ctrl+C again to quit")?;
                    }
                }
            }
        }
        if self.prompt.take().is_some() {
            println!();
        }
        Ok(())
    }

    fn spawn(&self, sub: Submission) -> JoinHandle<Result<(), CoreError>> {
        let session = self.session.clone();
        tokio::spawn(async move { session.submit(sub).await })
    }

    /// A line typed at `you: `: a prompt, or a slash command as typed text
    /// (plain mode has no pickers).
    fn input(&mut self, line: &str) -> std::io::Result<Next> {
        let text = line.trim();
        if text.is_empty() {
            return Ok(Next::Prompt);
        }
        let sub = match commands::parse(text, Tier::Code) {
            None => user_turn(text.to_string()),
            Some(Action::Submit(sub)) => sub,
            Some(Action::Mode(mode)) => Submission::SetPermissionMode { mode },
            Some(Action::Quit) => return Ok(Next::Quit),
            Some(Action::Help) => {
                // Plain mode reads whole lines, so no keybinding applies;
                // the default keys still say what the TUI would do.
                let keys = cox_tui::keymap::Keymap::default();
                self.line("help", &commands::help(&keys))?;
                return Ok(Next::Prompt);
            }
            Some(Action::Cost) => {
                self.line("cost", &format!("${:.4} this session", self.session_usd))?;
                return Ok(Next::Prompt);
            }
            Some(Action::Notice(text)) => {
                self.line("notice", &text)?;
                return Ok(Next::Prompt);
            }
            Some(_) => {
                let name = text.split_whitespace().next().unwrap_or(text);
                self.line("notice", &format!("{name} needs the full TUI"))?;
                return Ok(Next::Prompt);
            }
        };
        Ok(Next::Busy(self.spawn(sub)))
    }

    fn event(&mut self, ev: Event) -> std::io::Result<()> {
        match ev {
            Event::TurnStarted { model, .. } => {
                self.tui.status.model = model.to_string();
            }
            // T50.5: `/permissions` and `/effort` change the session; the
            // status line follows the event, as the TUI's does.
            Event::StateChanged { mode, effort } => {
                self.tui.mode = mode;
                self.tui.status.effort = effort;
            }
            Event::ItemStarted { item, kind } => {
                if let Some(entry) = buffered(kind, self.full_thinking) {
                    self.texts.insert(item, entry);
                }
            }
            Event::TextDelta { item, text } | Event::ThinkingDelta { item, text } => {
                if let Some((_, buf)) = self.texts.get_mut(&item) {
                    buf.push_str(&text);
                }
            }
            Event::ItemDone { item } => {
                if let Some((label, text)) = self.texts.remove(&item) {
                    let text = flatten_tables(&sanitize(&text));
                    if !text.trim().is_empty() {
                        self.line(label, text.trim())?;
                    }
                }
            }
            Event::ToolCallRequested { call } => {
                let what = sanitize(&format!("{} {}", call.name, call.subject));
                self.line("tool", what.trim())?;
            }
            Event::ToolCallDone { result, .. } => {
                let (label, rest) = result_lines(&result);
                self.line("result", &label)?;
                for row in rest {
                    self.raw(&row)?;
                }
            }
            Event::ApprovalRequired { call, .. } => self.push_ask(Ask::Approval(call.id))?,
            Event::QuestionAsked {
                call_id,
                question,
                options,
                source,
            } => self.push_ask(Ask::Question {
                call_id,
                text: question,
                options,
                from: source.and_then(|s| s.agent),
            })?,
            Event::Usage { usage, .. } => {
                self.turn_usd += usage.cost_usd;
                self.turn_tokens[0] += usage.context_tokens();
                self.turn_tokens[1] += usage.output_tokens;
                self.session_usd += usage.cost_usd;
                self.tui.status.cost_usd = self.session_usd;
                self.tui.status.context_tokens = usage.context_tokens();
                self.tui.status.cache_ratio = cox_core::cache_diag::ratio_of(&usage);
            }
            Event::Compacted {
                before_tokens,
                after_tokens,
                ..
            } => self.line(
                "notice",
                &format!("context compacted: {before_tokens} to {after_tokens} tokens"),
            )?,
            Event::ModelSwitched { from, to, .. } => {
                self.line("notice", &sanitize(&format!("model {from} to {to}")))?
            }
            Event::Notice { level, text } => self.line(level_label(level), &sanitize(&text))?,
            Event::Error { error, .. } => self.line("error", &sanitize(&error.to_string()))?,
            Event::TurnDone { stop, .. } => {
                if let Some(why) = stop_text(&stop) {
                    self.line("notice", &sanitize(&format!("stopped: {why}")))?;
                }
                let usd = std::mem::take(&mut self.turn_usd);
                let [input, output] = std::mem::take(&mut self.turn_tokens);
                self.line(
                    "cost",
                    &format!(
                        "${usd:.4} this turn, ${:.4} session, {input} in / {output} out tokens",
                        self.session_usd
                    ),
                )?;
                // T28.1: the same segments the TUI status line shows, once
                // per turn, so `--plain` carries the row a screen reader
                // cannot watch update in place.
                self.line("status", &cox_tui::status::plain_text(&self.tui))?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Queues `ask`; shown now when nothing else waits.
    fn push_ask(&mut self, ask: Ask) -> std::io::Result<()> {
        self.asks.push_back(ask);
        if self.asks.len() == 1 {
            self.show_ask()?;
        }
        Ok(())
    }

    /// Shows the oldest pending ask; a question prints its numbered options.
    fn show_ask(&mut self) -> std::io::Result<()> {
        let lines = match self.asks.front() {
            None => return Ok(()),
            Some(Ask::Approval(_)) => return self.show_prompt(APPROVE),
            Some(Ask::Question {
                text,
                options,
                from,
                ..
            }) => {
                let label = match from {
                    Some(from) => format!("question from {}", sanitize(from)),
                    None => "question".to_string(),
                };
                let mut lines = vec![format!("{label}: {}", sanitize(text))];
                for (i, option) in options.iter().enumerate() {
                    lines.push(format!("  [{}] {}", i + 1, sanitize(option)));
                }
                lines
            }
        };
        for line in lines {
            self.raw(&line)?;
        }
        self.show_prompt("answer: ")
    }

    async fn answer(&mut self, line: &str) -> anyhow::Result<()> {
        let text = line.trim();
        match self.asks.pop_front() {
            Some(Ask::Approval(call_id)) => {
                let decision = match text {
                    "1" => Decision::Allow,
                    "2" => Decision::AllowForSession,
                    "3" => Decision::Deny {
                        reason: "denied by the user".into(),
                    },
                    _ => {
                        self.asks.push_front(Ask::Approval(call_id));
                        self.line("notice", "type 1, 2 or 3")?;
                        return Ok(self.show_ask()?);
                    }
                };
                self.session
                    .submit(Submission::Approve { call_id, decision })
                    .await?;
            }
            // A number picks an option, anything else is the answer; an
            // empty line dismisses, which `ask_user` reports as unanswered.
            Some(Ask::Question {
                call_id, options, ..
            }) => {
                let picked = text
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .and_then(|i| options.get(i).cloned());
                let answer = picked.unwrap_or_else(|| text.to_string());
                self.session
                    .submit(Submission::Answer {
                        call_id,
                        text: (!answer.is_empty()).then_some(answer),
                    })
                    .await?;
            }
            None => {}
        }
        Ok(self.show_ask()?)
    }

    /// Interrupt or EOF: deny what waits for approval, dismiss questions.
    async fn cancel_asks(&mut self) -> anyhow::Result<()> {
        for ask in std::mem::take(&mut self.asks) {
            let sub = match ask {
                Ask::Approval(call_id) => Submission::Approve {
                    call_id,
                    decision: Decision::Deny {
                        reason: "interrupted before a decision".to_string(),
                    },
                },
                Ask::Question { call_id, .. } => Submission::Answer {
                    call_id,
                    text: None,
                },
            };
            self.session.submit(sub).await?;
        }
        Ok(())
    }

    /// BEL, then the prompt; nothing after it until the user answers.
    fn show_prompt(&mut self, prompt: &'static str) -> std::io::Result<()> {
        let mut out = std::io::stdout().lock();
        write!(out, "\x07{prompt}")?;
        out.flush()?;
        self.prompt = Some(prompt);
        Ok(())
    }

    fn line(&mut self, label: &str, text: &str) -> std::io::Result<()> {
        self.raw(&format!("{label}: {text}"))
    }

    /// Appends `text` as whole lines; a prompt already on screen ends its
    /// line first and is printed again below, never moved back to.
    fn raw(&mut self, text: &str) -> std::io::Result<()> {
        let mut out = std::io::stdout().lock();
        if self.prompt.is_some() {
            writeln!(out)?;
        }
        writeln!(out, "{text}")?;
        if let Some(prompt) = self.prompt {
            write!(out, "{prompt}")?;
        }
        out.flush()
    }
}

fn user_turn(text: String) -> Submission {
    Submission::UserTurn {
        text,
        attachments: Vec::new(),
        confirm_think: false,
    }
}

fn level_label(level: Level) -> &'static str {
    match level {
        Level::Info => "notice",
        Level::Warn => "warning",
        Level::Budget => "budget",
        Level::Security => "security",
    }
}

fn stop_text(stop: &StopReason) -> Option<String> {
    Some(match stop {
        StopReason::EndTurn => return None,
        StopReason::MaxTurns => "max turns reached".into(),
        StopReason::Interrupted => "interrupted".into(),
        StopReason::Budget => "budget cap reached".into(),
        StopReason::Refusal { detail } => format!("refused: {detail}"),
        StopReason::Error => "error".into(),
    })
}

/// `ok, N lines` plus the first and last [`HEAD_TAIL`] lines, indented.
fn result_lines(result: &ToolResult) -> (String, Vec<String>) {
    let text = sanitize(&result.visible);
    let lines: Vec<&str> = text.lines().collect();
    let state = if result.ok { "ok" } else { "failed" };
    let plural = if lines.len() == 1 { "" } else { "s" };
    let mut label = format!("{state}, {} line{plural}", lines.len());
    // Every result is archived; only a shortened one needs pointing at.
    if let Some(archive) = result
        .archive
        .as_ref()
        .filter(|_| result.bytes > result.visible.len() as u64)
    {
        label.push_str(&format!(", full output: cox expand {}", archive.id));
    }
    let indent = |l: &&str| format!("  {l}");
    let rest = if lines.len() <= 2 * HEAD_TAIL {
        lines.iter().map(indent).collect()
    } else {
        let hidden = lines.len() - 2 * HEAD_TAIL;
        let mut rest: Vec<String> = lines[..HEAD_TAIL].iter().map(indent).collect();
        rest.push(format!("  ({hidden} more lines)"));
        rest.extend(lines[lines.len() - HEAD_TAIL..].iter().map(indent));
        rest
    };
    (label, rest)
}

/// A markdown table becomes one `Header: value; Header: value` line per
/// row; a screen reader reading pipes and dashes is noise.
fn flatten_tables(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        if !(is_row(lines[i]) && lines.get(i + 1).is_some_and(|l| is_rule(l))) {
            out.push(lines[i].to_string());
            i += 1;
            continue;
        }
        let head = cells(lines[i]);
        i += 2;
        while i < lines.len() && is_row(lines[i]) {
            let row: Vec<String> = head
                .iter()
                .zip(cells(lines[i]))
                .map(|(h, v)| format!("{h}: {v}"))
                .collect();
            out.push(row.join("; "));
            i += 1;
        }
    }
    out.join("\n")
}

fn is_row(line: &str) -> bool {
    let t = line.trim();
    t.len() > 1 && t.starts_with('|')
}

fn is_rule(line: &str) -> bool {
    is_row(line)
        && line.contains('-')
        && line
            .trim()
            .chars()
            .all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

fn cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|c| c.trim().to_string())
        .collect()
}

/// Pends forever without a running submission so `select!` never spins.
async fn join(
    busy: &mut Option<JoinHandle<Result<(), CoreError>>>,
) -> Result<Result<(), CoreError>, tokio::task::JoinError> {
    match busy {
        Some(handle) => handle.await,
        None => std::future::pending().await,
    }
}

/// stdin blocks, so a thread turns it into lines; `None` is EOF.
fn spawn_stdin() -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel(16);
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.blocking_send(line).is_err() {
                break;
            }
        }
    });
    rx
}

/// One long-lived listener, so a `Ctrl+C` between two `select!` rounds is
/// not lost to a re-registered handler.
fn spawn_sigint() -> mpsc::Receiver<()> {
    let (tx, rx) = mpsc::channel(4);
    tokio::spawn(async move {
        while tokio::signal::ctrl_c().await.is_ok() {
            if tx.send(()).await.is_err() {
                break;
            }
        }
    });
    rx
}

/// The label and opening text of a started item that is printed whole at
/// `ItemDone`; `None` for an item shown some other way or not at all.
fn buffered(kind: ItemKind, full_thinking: bool) -> Option<(&'static str, String)> {
    match kind {
        ItemKind::AssistantMessage { text } => Some(("cox", text)),
        // T39.2 keeps a tool call's signature as an empty signed item for
        // the provider's history: a replay token, not something the model
        // said. A streamed thought also starts empty, but unsigned.
        ItemKind::Thinking {
            text,
            signature: Some(_),
        } if text.is_empty() => None,
        ItemKind::Thinking { text, .. } if full_thinking => Some(("thinking", text)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_skips_empty_signed_thinking() {
        let thinking = |text: &str, signature: Option<&str>| ItemKind::Thinking {
            text: text.into(),
            signature: signature.map(Into::into),
        };
        assert_eq!(buffered(thinking("", Some("sig")), true), None);
        assert_eq!(
            buffered(thinking("", None), true),
            Some(("thinking", String::new()))
        );
        assert_eq!(
            buffered(thinking("why", Some("sig")), true),
            Some(("thinking", "why".into()))
        );
        assert_eq!(buffered(thinking("why", None), false), None);
    }

    #[test]
    fn markdown_table_reads_as_header_value_rows() {
        let text = "Files:\n| file | state |\n|:---|---:|\n| a.txt | new |\n| b.rs | edited |\nend";
        assert_eq!(
            flatten_tables(text),
            "Files:\nfile: a.txt; state: new\nfile: b.rs; state: edited\nend"
        );
    }

    #[test]
    fn pipe_lines_without_a_rule_are_left_alone() {
        let text = "| not a table\nplain";
        assert_eq!(flatten_tables(text), text);
    }

    #[test]
    fn long_tool_result_shows_head_and_tail_only() {
        let visible: Vec<String> = (1..=10).map(|n| format!("line {n}")).collect();
        let result = ToolResult {
            ok: true,
            visible: visible.join("\n"),
            archive: None,
            bytes: 0,
            duration_ms: 0,
            diff: None,
            structured: None,
        };
        let (label, rest) = result_lines(&result);
        assert_eq!(label, "ok, 10 lines");
        assert_eq!(
            rest,
            [
                "  line 1",
                "  line 2",
                "  line 3",
                "  (4 more lines)",
                "  line 8",
                "  line 9",
                "  line 10"
            ]
        );
    }

    #[test]
    fn tool_result_escape_sequences_are_stripped() {
        let result = ToolResult {
            ok: false,
            visible: "\x1b[2J\x1b[Hboom".into(),
            archive: None,
            bytes: 0,
            duration_ms: 0,
            diff: None,
            structured: None,
        };
        let (label, rest) = result_lines(&result);
        assert_eq!(label, "failed, 1 line");
        assert_eq!(rest, ["  boom"]);
    }

    /// T50.5 Check: `/permissions plan` reaches the status line through the
    /// session's `StateChanged`, not the configured mode.
    #[tokio::test]
    async fn permissions_command_updates_the_plain_status_mode() {
        let home = tempfile::tempdir().expect("home");
        let work = tempfile::tempdir().expect("work");
        let scenario = "[[turn]]\ntext = \"ok\"\n";
        let (session, _store) =
            cox_session::testing::scripted_session(home.path(), work.path(), scenario);
        let mut events = session.events().expect("events");
        let config = cox_protocol::Config::default();
        let mut plain = Plain {
            session,
            full_thinking: false,
            texts: HashMap::new(),
            asks: VecDeque::new(),
            prompt: None,
            armed: false,
            turn_usd: 0.0,
            turn_tokens: [0, 0],
            session_usd: 0.0,
            tui: cox_tui::state::State::new(config.permissions.mode, config.sandbox.mode),
        };
        let before = cox_tui::status::plain_text(&plain.tui);
        assert!(!before.contains("plan"), "{before}");
        plain.input("/permissions plan").expect("input");
        let changed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match events.recv().await {
                    Some(ev @ Event::StateChanged { .. }) => break ev,
                    Some(_) => {}
                    None => panic!("event stream closed"),
                }
            }
        })
        .await
        .expect("StateChanged");
        plain.event(changed).expect("event");
        let after = cox_tui::status::plain_text(&plain.tui);
        assert!(after.contains("plan"), "{after}");
    }
}
