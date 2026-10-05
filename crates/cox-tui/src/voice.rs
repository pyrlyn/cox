// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Push-to-talk (T54.6, A123): the `voice` key starts and stops a
//! recording, `Esc` cancels it, and the transcript lands in the composer.
//! `Voice` and `on_key`/`on_msg` are the pure half `state::update` calls;
//! `Driver` is the half `app::run` owns, the one place a `Dictation` is
//! called, on its own task so a transcription never stalls the screen.
//! Separate from `state` so the whole feature reads in one file and its
//! tests need a fake `Dictation`, not a microphone.

use cox_protocol::traits::Dictation;
use cox_protocol::types::Level;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio::sync::mpsc;

use crate::commands::Context;
use crate::keymap::{self, Action};
use crate::state::{Cmd, Msg, State};

/// What the runtime does with the `Dictation` it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceCmd {
    Start,
    Stop,
    Cancel,
}

/// What the `Dictation` answered, back on `Driver::recv`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceMsg {
    /// The recording could not start (no device, no permission).
    Failed(String),
    /// `stop`'s answer: the transcript, or why there is none.
    Transcript(Result<String, String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Idle,
    /// Since `tick`; `empty` is whether the draft was empty at the press,
    /// the one case a transcript may submit itself.
    Recording {
        since: u64,
        empty: bool,
    },
    Transcribing {
        empty: bool,
    },
}

/// Push-to-talk's part of `State`; the binary sets `auto_submit` from
/// `[voice]`, `app::run` sets `available` from the `Dictation` it got.
#[derive(Debug, Clone, Default)]
pub struct Voice {
    pub available: bool,
    pub auto_submit: bool,
    pub phase: Phase,
}

/// `Some` for a key push-to-talk owns ahead of the keymap: every release
/// (the only ones `app.rs` forwards are for this), a held voice key's
/// repeats, and `Esc` while recording.
pub fn on_key(state: &mut State, key: KeyEvent) -> Option<Vec<Cmd>> {
    let recording = matches!(state.voice.phase, Phase::Recording { .. });
    match key.kind {
        // A terminal that reports releases (kitty, T23.1) records while
        // the key is held; the modifier may already be up, so only the
        // key itself is compared.
        KeyEventKind::Release => Some(if recording && is_voice_code(state, key.code) {
            stop(state)
        } else {
            Vec::new()
        }),
        KeyEventKind::Repeat if is_voice_key(state, key) => Some(Vec::new()),
        _ if recording && key.code == KeyCode::Esc => {
            state.voice.phase = Phase::Idle;
            Some(vec![Cmd::Voice(VoiceCmd::Cancel)])
        }
        _ => None,
    }
}

/// The keymap's `voice` action: start, or stop a recording in progress.
pub fn toggle(state: &mut State) -> Vec<Cmd> {
    if !state.voice.available {
        notice(
            state,
            Level::Warn,
            "push-to-talk is off: set `[voice] enabled = true` in a build with the `voice` \
             feature, and get a model with `cox voice model download`",
        );
        return Vec::new();
    }
    match state.voice.phase {
        Phase::Idle => {
            state.voice.phase = Phase::Recording {
                since: state.tick,
                empty: state.composer.is_empty(),
            };
            vec![Cmd::Voice(VoiceCmd::Start)]
        }
        Phase::Recording { .. } => stop(state),
        Phase::Transcribing { .. } => Vec::new(),
    }
}

fn stop(state: &mut State) -> Vec<Cmd> {
    let Phase::Recording { empty, .. } = state.voice.phase else {
        return Vec::new();
    };
    state.voice.phase = Phase::Transcribing { empty };
    vec![Cmd::Voice(VoiceCmd::Stop)]
}

/// The transcript into the composer at the cursor, sent as `Enter` would
/// only when the draft was empty at the press and still is, so text typed
/// meanwhile or already there is always reviewed first.
pub fn on_msg(state: &mut State, msg: VoiceMsg) -> Vec<Cmd> {
    let was_empty = match std::mem::take(&mut state.voice.phase) {
        Phase::Transcribing { empty } | Phase::Recording { empty, .. } => empty,
        // A cancelled recording's late answer.
        Phase::Idle => return Vec::new(),
    };
    let text = match msg {
        VoiceMsg::Failed(why) => {
            notice(state, Level::Warn, &format!("push-to-talk: {why}"));
            return Vec::new();
        }
        VoiceMsg::Transcript(Err(why)) => {
            notice(state, Level::Warn, &format!("transcription failed: {why}"));
            return Vec::new();
        }
        VoiceMsg::Transcript(Ok(text)) => crate::text::sanitize(&text).trim().to_string(),
    };
    if text.is_empty() {
        notice(state, Level::Info, "no speech heard");
        return Vec::new();
    }
    let submit = was_empty && state.voice.auto_submit && state.composer.is_empty();
    state.composer.insert(&text);
    if submit {
        crate::state::compose(state, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    } else {
        Vec::new()
    }
}

/// The status row's push-to-talk segment, while there is one.
pub fn status(state: &State) -> Option<String> {
    match state.voice.phase {
        Phase::Idle => None,
        Phase::Recording { since, .. } => {
            let secs = state.tick.saturating_sub(since) / 10;
            Some(format!("● rec {}:{:02}", secs / 60, secs % 60))
        }
        Phase::Transcribing { .. } => Some("transcribing…".into()),
    }
}

fn notice(state: &mut State, level: Level, text: &str) {
    state.transcript.push(crate::state::Cell::Notice {
        level,
        text: text.to_string(),
    });
}

fn is_voice_key(state: &State, key: KeyEvent) -> bool {
    state.keymap.resolve(key, Context::Idle) == Some(Action::Voice)
}

fn is_voice_code(state: &State, code: KeyCode) -> bool {
    let same = |a: KeyCode, b: KeyCode| match (a, b) {
        (KeyCode::Char(a), KeyCode::Char(b)) => a.eq_ignore_ascii_case(&b),
        (a, b) => a == b,
    };
    state
        .keymap
        .rows(Context::Idle)
        .iter()
        .filter(|(_, action)| *action == Action::Voice.id())
        .filter_map(|(shown, _)| keymap::parse(shown))
        .any(|(bound, _)| same(bound, code))
}

/// The runtime's side: owns the `Dictation` on a task that runs one
/// command at a time and answers on `recv`. Without a `Dictation` it never
/// answers, and `toggle` never asks.
pub struct Driver {
    cmds: Option<mpsc::UnboundedSender<VoiceCmd>>,
    msgs: mpsc::UnboundedReceiver<Msg>,
    /// Keeps `msgs` open when there is no task, so `recv` stays pending.
    _open: mpsc::UnboundedSender<Msg>,
}

impl Driver {
    pub fn new(dictation: Option<Box<dyn Dictation>>) -> Self {
        let (tx, msgs) = mpsc::unbounded_channel();
        let cmds = dictation.map(|mut dictation| {
            let (cmds, mut rx) = mpsc::unbounded_channel();
            let tx = tx.clone();
            tokio::spawn(async move {
                while let Some(cmd) = rx.recv().await {
                    let answer = match cmd {
                        VoiceCmd::Start => dictation
                            .start()
                            .err()
                            .map(|e| VoiceMsg::Failed(e.to_string())),
                        VoiceCmd::Stop => Some(VoiceMsg::Transcript(
                            dictation.stop().await.map_err(|e| e.to_string()),
                        )),
                        VoiceCmd::Cancel => {
                            dictation.cancel();
                            None
                        }
                    };
                    if let Some(answer) = answer
                        && tx.send(Msg::Voice(answer)).is_err()
                    {
                        return;
                    }
                }
            });
            cmds
        });
        Self {
            cmds,
            msgs,
            _open: tx,
        }
    }

    pub fn send(&self, cmd: VoiceCmd) {
        if let Some(cmds) = &self.cmds {
            let _ = cmds.send(cmd);
        }
    }

    pub async fn recv(&mut self) -> Option<Msg> {
        self.msgs.recv().await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use cox_protocol::traits::DictationError;
    use cox_protocol::types::{PermissionMode, SandboxMode, Submission};

    use super::*;
    use crate::state::{Cell, update};

    /// A `Dictation` that says `said` and logs each call.
    struct Fake {
        said: String,
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait::async_trait]
    impl Dictation for Fake {
        fn start(&mut self) -> Result<(), DictationError> {
            self.log.lock().expect("log").push("start");
            Ok(())
        }
        async fn stop(&mut self) -> Result<String, DictationError> {
            self.log.lock().expect("log").push("stop");
            Ok(self.said.clone())
        }
        fn cancel(&mut self) {
            self.log.lock().expect("log").push("cancel");
        }
    }

    fn state() -> State {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        state.voice.available = true;
        state.voice.auto_submit = true;
        state
    }

    fn fake(said: &str) -> (Driver, Arc<Mutex<Vec<&'static str>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let dictation = Fake {
            said: said.into(),
            log: Arc::clone(&log),
        };
        (Driver::new(Some(Box::new(dictation))), log)
    }

    fn alt_v(kind: KeyEventKind) -> Msg {
        let mut key = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT);
        key.kind = kind;
        Msg::Key(key)
    }

    /// Feeds `msg` to `update`, runs its voice commands on `driver` and
    /// feeds back each answer; returns the other commands.
    async fn drive(state: &mut State, driver: &mut Driver, msg: Msg) -> Vec<Cmd> {
        let mut rest = Vec::new();
        let mut queue = vec![msg];
        while let Some(msg) = queue.pop() {
            for cmd in update(state, msg) {
                match cmd {
                    Cmd::Voice(cmd) => {
                        driver.send(cmd);
                        if cmd == VoiceCmd::Stop {
                            queue.push(driver.recv().await.expect("answer"));
                        }
                    }
                    other => rest.push(other),
                }
            }
        }
        rest
    }

    fn submitted(cmds: &[Cmd]) -> Option<String> {
        cmds.iter().find_map(|c| match c {
            Cmd::Submit(Submission::UserTurn { text, .. }) => Some(text.clone()),
            _ => None,
        })
    }

    #[tokio::test]
    async fn voice_key_starts_and_stops_recording() {
        let (mut driver, log) = fake("fix the flaky test");
        let mut state = state();
        state.composer.insert("please ");
        drive(&mut state, &mut driver, alt_v(KeyEventKind::Press)).await;
        assert!(matches!(state.voice.phase, Phase::Recording { .. }));
        let cmds = drive(&mut state, &mut driver, alt_v(KeyEventKind::Press)).await;
        assert_eq!(state.voice.phase, Phase::Idle);
        assert_eq!(*log.lock().expect("log"), ["start", "stop"]);
        assert_eq!(state.composer.text(), "please fix the flaky test");
        assert!(submitted(&cmds).is_none());
    }

    #[tokio::test]
    async fn voice_release_stops_when_the_terminal_reports_releases() {
        let (mut driver, log) = fake("hello");
        let mut state = state();
        state.voice.auto_submit = false;
        drive(&mut state, &mut driver, alt_v(KeyEventKind::Press)).await;
        drive(&mut state, &mut driver, alt_v(KeyEventKind::Repeat)).await;
        assert!(matches!(state.voice.phase, Phase::Recording { .. }));
        // Alt already let go: the release carries no modifier.
        let release = KeyEvent::new_with_kind(
            KeyCode::Char('v'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        drive(&mut state, &mut driver, Msg::Key(release)).await;
        assert_eq!(*log.lock().expect("log"), ["start", "stop"]);
        assert_eq!(state.composer.text(), "hello");
    }

    #[tokio::test]
    async fn voice_escape_cancels_without_inserting() {
        let (mut driver, log) = fake("said");
        let mut state = state();
        state.status.busy = true;
        drive(&mut state, &mut driver, alt_v(KeyEventKind::Press)).await;
        let esc = Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let cmds = drive(&mut state, &mut driver, esc).await;
        assert!(cmds.is_empty(), "Esc must not also interrupt: {cmds:?}");
        assert_eq!(state.voice.phase, Phase::Idle);
        assert!(state.composer.is_empty() && state.queue.is_empty());
        // The task runs commands in order; a later start proves cancel ran.
        drive(&mut state, &mut driver, alt_v(KeyEventKind::Press)).await;
        drive(&mut state, &mut driver, alt_v(KeyEventKind::Press)).await;
        assert_eq!(
            *log.lock().expect("log"),
            ["start", "cancel", "start", "stop"]
        );
        // A cancelled recording's late answer inserts nothing.
        let mut state = self::state();
        assert!(on_msg(&mut state, VoiceMsg::Transcript(Ok("late".into()))).is_empty());
        assert!(state.composer.is_empty());
    }

    #[tokio::test]
    async fn voice_transcript_auto_submits_an_empty_draft() {
        let (mut driver, _) = fake("  run the tests  ");
        let mut state = state();
        drive(&mut state, &mut driver, alt_v(KeyEventKind::Press)).await;
        let cmds = drive(&mut state, &mut driver, alt_v(KeyEventKind::Press)).await;
        let sent = submitted(&cmds).expect("submitted");
        assert!(sent.contains("run the tests"), "{sent}");
        assert!(state.composer.is_empty());

        // While a turn runs it queues, as `Enter` does.
        let (mut driver, _) = fake("and then lint");
        state.status.busy = true;
        drive(&mut state, &mut driver, alt_v(KeyEventKind::Press)).await;
        let cmds = drive(&mut state, &mut driver, alt_v(KeyEventKind::Press)).await;
        assert!(submitted(&cmds).is_none());
        assert_eq!(
            state.queue.back().map(String::as_str),
            Some("and then lint")
        );
    }

    #[tokio::test]
    async fn voice_transcript_into_a_non_empty_draft_does_not_submit() {
        let (mut driver, _) = fake("the parser");
        let mut state = state();
        state.composer.insert("refactor ");
        drive(&mut state, &mut driver, alt_v(KeyEventKind::Press)).await;
        let cmds = drive(&mut state, &mut driver, alt_v(KeyEventKind::Press)).await;
        assert!(submitted(&cmds).is_none());
        assert_eq!(state.composer.text(), "refactor the parser");

        // Typed during the recording: the draft is no longer empty.
        let mut state = self::state();
        drive(&mut state, &mut driver, alt_v(KeyEventKind::Press)).await;
        state.composer.insert("x ");
        let cmds = drive(&mut state, &mut driver, alt_v(KeyEventKind::Press)).await;
        assert!(submitted(&cmds).is_none());
    }

    #[tokio::test]
    async fn voice_transcript_is_sanitized() {
        let (mut driver, _) = fake("hi\u{1b}[31m there\u{202e}");
        let mut state = state();
        state.voice.auto_submit = false;
        drive(&mut state, &mut driver, alt_v(KeyEventKind::Press)).await;
        drive(&mut state, &mut driver, alt_v(KeyEventKind::Press)).await;
        let text = state.composer.text();
        assert!(
            !text.contains('\u{1b}') && !text.contains('\u{202e}'),
            "{text:?}"
        );

        // Nothing but whitespace: a dim notice, nothing inserted.
        let (mut driver, _) = fake("  \n ");
        let mut state = self::state();
        drive(&mut state, &mut driver, alt_v(KeyEventKind::Press)).await;
        let cmds = drive(&mut state, &mut driver, alt_v(KeyEventKind::Press)).await;
        assert!(cmds.is_empty() && state.composer.is_empty());
        assert!(matches!(
            state.transcript.last(),
            Some(Cell::Notice { level: Level::Info, text }) if text == "no speech heard"
        ));
    }

    #[test]
    fn voice_without_dictation_shows_a_notice() {
        let mut state = State::new(PermissionMode::Default, SandboxMode::WorkspaceWrite);
        let cmds = update(&mut state, alt_v(KeyEventKind::Press));
        assert!(cmds.iter().all(|c| !matches!(c, Cmd::Voice(_))));
        assert_eq!(state.voice.phase, Phase::Idle);
        assert!(matches!(
            state.transcript.last(),
            Some(Cell::Notice { level: Level::Warn, text }) if text.contains("cox voice")
        ));
    }

    #[test]
    fn voice_recording_status_row() {
        let mut state = state();
        state.status.model = "sonnet-5".into();
        update(&mut state, alt_v(KeyEventKind::Press));
        for _ in 0..72 {
            update(&mut state, Msg::Tick);
        }
        let recording = crate::status::line_at(&state, 200).to_string();
        update(&mut state, alt_v(KeyEventKind::Press));
        let transcribing = crate::status::line_at(&state, 200).to_string();
        insta::assert_snapshot!(format!("{recording}\n{transcribing}"));
    }

    #[test]
    fn voice_key_is_in_the_help() {
        let rows = keymap::Keymap::default();
        assert!(rows.rows(Context::Idle).contains(&("Alt+V", "voice")));
    }
}
