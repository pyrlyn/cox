// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The runtime: crossterm input (polled on a thread) and core `Event`s on
//! one `select!`, an
//! inline viewport, `insert_before` for finished cells so the terminal's own
//! scrollback keeps the transcript, a resize that waits for the size to
//! settle and then rebuilds the viewport where it was (T23.7), and a panic
//! hook that restores the terminal. The only module in the crate that touches a real terminal;
//! everything it decides goes through `state::update`.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cox_core::Session;
use cox_protocol::errors::CoreError;
use crossterm::cursor::MoveTo;
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, Event as Input, KeyEventKind, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Size;
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{Terminal, TerminalOptions, Viewport};

use crate::cells::cell_lines;
use crate::state::{Ask, Cmd, GrantDecision, Msg, PluginMgmtRequest, PluginRequest, State, update};
use crate::view::view;
use crate::voice::{Driver, Phase};

/// Rows the live viewport keeps below the scrollback; a short terminal
/// gets two fewer than its height so some scrollback stays visible.
const VIEWPORT_ROWS: u16 = 15;

/// A resize arrives as a burst of size changes while a window is dragged;
/// acting on each would stack stale viewports in scrollback, so the
/// viewport is rebuilt only once two size reads this far apart agree.
const SETTLE: Duration = Duration::from_millis(16);

type Term = Terminal<CrosstermBackend<io::Stdout>>;

/// Why the TUI stopped; the binary uses this to quit or start a fresh session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TuiOutcome {
    Quit,
    Clear,
    /// `/fork [turn]` (T26.3): continue in a child with the history up to `turn`.
    Fork {
        turn: Option<u32>,
    },
    /// `/handoff <objective>` (T26.3): continue in a summary-seeded child.
    Handoff {
        objective: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum TuiError {
    #[error("terminal: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("the session's event stream was already taken")]
    EventsTaken,
}

/// Runs the TUI until the user quits or the session's stream ends. The
/// caller fills `state.files` (the `@` picker's candidates) from
/// `cox_tools::glob::workspace_files` — this crate never walks the disk.
/// `feed` carries what the runtime learns off-screen (the live sessions of
/// this workspace, T16.3; git counts, T15.2) for the same reason, and
/// `ask` carries what the TUI wants fetched (the diff, T15.3); the answer
/// arrives on `feed`. `persist`
/// carries a `/theme` choice's `(key, value)` (T24.2) out to `config_cmd::set`
/// — this crate has no `toml_edit`-editing path of its own. `grants`
/// (T33.8) carries a `Modal::PluginGrant`'s `y` out to `crates/cox`, which
/// writes it — same reason as `persist`, this crate never touches the store.
/// `plugins` (T33.23) carries each `Cmd::Plugin` to `crates/cox`, which holds
/// the plugin hosts; the answer arrives on `feed` as `Msg::Plugin`.
/// `plugin_mgmt` (T33.30 `New`; T33.33 `Update`/`Remove`/`List`) carries a
/// `Cmd::PluginMgmt` the same way; its answer arrives on `feed` as
/// `Msg::PluginMgmt`. `dictation` (T54.6) is push-to-talk's speech-to-text,
/// `None` when the build or `[voice]` leaves it off; the voice key then
/// only says how to turn it on.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    session: Session,
    mut state: State,
    mut feed: tokio::sync::mpsc::Receiver<Msg>,
    ask: tokio::sync::mpsc::Sender<Ask>,
    persist: tokio::sync::mpsc::Sender<(String, String)>,
    grants: tokio::sync::mpsc::Sender<GrantDecision>,
    plugins: tokio::sync::mpsc::Sender<PluginRequest>,
    plugin_mgmt: tokio::sync::mpsc::Sender<PluginMgmtRequest>,
    dictation: Option<Box<dyn cox_protocol::traits::Dictation>>,
) -> Result<TuiOutcome, TuiError> {
    let mut rx = session.events().ok_or(TuiError::EventsTaken)?;
    state.voice.available = dictation.is_some();
    let mut voice = Driver::new(dictation);
    enable_raw_mode()?;
    execute!(io::stdout(), EnableBracketedPaste)?;
    // T23.1: only a terminal `cox_tui::term::Caps::query` already found to
    // report `CSI ?u` support gets the push — everything else keeps the
    // plain `Esc`-prefixed encoding it always had.
    let kitty = state.caps.kitty_keyboard;
    if kitty {
        execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            )
        )?;
    }
    // T23.5: focus reports decide whether `tui.notify = auto` rings.
    let focus = state.caps.focus;
    // T23.6: a quit mid-turn must not leave the tab spinning.
    let progress = state.caps.osc9_4;
    if focus {
        execute!(io::stdout(), EnableFocusChange)?;
    }
    // T22.4: `tui.mouse` is the only switch — no in-app toggle key — so a
    // terminal that never asked for reports keeps its own text selection.
    let mouse = state.mouse;
    if mouse {
        execute!(io::stdout(), EnableMouseCapture)?;
    }
    let vte = crate::term::is_vte(&|k| std::env::var(k).ok());
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore(kitty, focus, progress, mouse);
        hook(info);
    }));
    let mut terminal = inline_terminal(crossterm::terminal::size()?.1)?;
    let mut built_for = terminal.size()?;
    // T33.24, PL§8: seed `state.term` with the real startup size, the same
    // one the first draw is built for — otherwise a `panel`/`overlay`
    // opened before the first `Msg::Resize` would ask a plugin to render
    // into `State::new`'s placeholder area instead of the real one.
    state.term = (built_for.width, built_for.height);
    // The last size read while a resize settles, and when it was taken.
    let mut settling: Option<(Size, Instant)> = None;
    // The cursor's row inside the viewport after the last draw: after a
    // resize the terminal has moved the cursor along with its line, so this
    // is how far above it the old viewport starts.
    let mut cursor_row = 0;
    let stop = Arc::new(AtomicBool::new(false));
    let mut input = spawn_input(stop.clone());
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let result = async {
        loop {
            let msg = tokio::select! {
                Some(ev) = input.recv() => match ev? {
                    // T54.6: a release matters only to a held voice key.
                    Input::Key(k)
                        if k.kind != KeyEventKind::Release
                            || matches!(state.voice.phase, Phase::Recording { .. }) =>
                    {
                        Msg::Key(k)
                    }
                    Input::Paste(text) => Msg::Paste(text),
                    Input::Resize(w, h) => Msg::Resize(w, h),
                    Input::FocusGained => Msg::Focus(true),
                    Input::FocusLost => Msg::Focus(false),
                    Input::Mouse(m) => Msg::Mouse(m),
                    _ => continue,
                },
                ev = rx.recv() => match ev {
                    Some(ev) => Msg::Event(ev),
                    None => return Ok(TuiOutcome::Quit),
                },
                _ = tick.tick() => Msg::Tick,
                Some(msg) = feed.recv() => msg,
                Some(msg) = voice.recv() => msg,
            };
            let ticked = matches!(msg, Msg::Tick);
            for cmd in update(&mut state, msg) {
                match cmd {
                    // Same reason as the headless loop: `submit` runs a whole
                    // turn, and an approval must be answerable meanwhile.
                    Cmd::Submit(sub) => {
                        let session = session.clone();
                        tokio::spawn(async move {
                            // Failures surface as `Event::Error` on the stream.
                            let _ = session.submit(sub).await;
                        });
                    }
                    Cmd::Quit => return Ok(TuiOutcome::Quit),
                    Cmd::Clear => return Ok(TuiOutcome::Clear),
                    Cmd::Fork(turn) => return Ok(TuiOutcome::Fork { turn }),
                    Cmd::Handoff(objective) => return Ok(TuiOutcome::Handoff { objective }),
                    // T23.4: `state` only ever emits this when
                    // `caps.osc52`, so no capability check is needed here —
                    // `app.rs` just writes the bytes `state` decided on.
                    Cmd::Copy(text) => {
                        use std::io::Write;
                        let mut out = io::stdout();
                        out.write_all(crate::term::copy(&text).as_bytes())?;
                        out.flush()?;
                    }
                    // A request the runtime has not answered yet is still
                    // pending, so a repeat is dropped rather than awaited.
                    Cmd::Ask(what) => {
                        let _ = ask.try_send(what);
                    }
                    // Best-effort: a full channel or a closed receiver just
                    // means this one preview is not persisted; the picker
                    // already applied it to `state` either way.
                    Cmd::PersistConfig { key, value } => {
                        let _ = persist.try_send((key, value));
                    }
                    Cmd::Progress(p) => {
                        use std::io::Write;
                        let mut out = io::stdout();
                        out.write_all(crate::term::progress(p).as_bytes())?;
                        out.flush()?;
                    }
                    Cmd::Notify { title, body } => {
                        use std::io::Write;
                        let bytes = crate::term::notification(&state.caps, vte, &title, &body);
                        let mut out = io::stdout();
                        out.write_all(bytes.as_bytes())?;
                        out.flush()?;
                    }
                    // T33.8: best-effort, like `PersistConfig` — a full
                    // channel or a closed receiver just means this one
                    // grant is not persisted; the dialog already moved on.
                    Cmd::PluginGrant(decision) => {
                        let _ = grants.try_send(decision);
                    }
                    // T33.23: best-effort too — a request lost to a full
                    // channel leaves the last good render on screen, and
                    // the next redraw or resize asks again.
                    Cmd::Plugin(request) => {
                        let _ = plugins.try_send(request);
                    }
                    // T33.30, T33.33: best-effort too — a request lost to a
                    // full channel just means the `/plugin` subcommand
                    // silently does nothing; the user can retry.
                    Cmd::PluginMgmt(request) => {
                        let _ = plugin_mgmt.try_send(request);
                    }
                    Cmd::Voice(cmd) => voice.send(cmd),
                }
            }
            // While a resize settles nothing is drawn or inserted: finished
            // cells wait in `state`, and ratatui's own `autoresize` — which
            // clears the whole screen when the width shrinks — never runs.
            let size = terminal.size()?;
            if size != built_for || settling.is_some() {
                settling = match settling {
                    Some((last, at)) if last == size && ticked && at.elapsed() >= SETTLE => {
                        terminal = rebuild(&mut terminal, cursor_row, size.height)?;
                        built_for = size;
                        None
                    }
                    Some((last, at)) if last == size => Some((last, at)),
                    _ => Some((size, Instant::now())),
                };
                if settling.is_some() {
                    continue;
                }
            }
            let look = state.look(size.width);
            let depth = state.depth;
            for cell in state.take_finished() {
                let lines = cell_lines(&cell, &look);
                let height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
                // Scrollback goes through the same colour mapping as the
                // viewport; it is written straight to the terminal.
                terminal.insert_before(height, |buf| {
                    Paragraph::new(lines).render(buf.area, buf);
                    crate::link::apply(buf, &state.cwd, state.caps.osc8);
                    crate::color::map_buffer(buf, depth);
                })?;
            }
            let mut drawn = None;
            terminal.draw(|frame| {
                let area = frame.area();
                let pos = view(&state, area, frame.buffer_mut());
                if let Some(pos) = pos {
                    frame.set_cursor_position(pos);
                }
                drawn = Some((area, pos));
            })?;
            match drawn {
                Some((area, Some(pos))) => cursor_row = pos.y.saturating_sub(area.y),
                // A hidden cursor stays wherever the diff ended; parking it
                // on the viewport's first row keeps `cursor_row` true.
                Some((area, None)) => {
                    terminal.set_cursor_position(area.as_position())?;
                    cursor_row = 0;
                }
                None => {}
            }
        }
    }
    .await;
    stop.store(true, Ordering::Relaxed);
    restore(kitty, focus, progress, mouse);
    result
}

/// An inline terminal whose viewport fits a screen `height` rows tall.
fn inline_terminal(height: u16) -> io::Result<Term> {
    let rows = VIEWPORT_ROWS.min(height.saturating_sub(2)).max(1);
    Terminal::with_options(
        CrosstermBackend::new(io::stdout()),
        TerminalOptions {
            viewport: Viewport::Inline(rows),
        },
    )
}

/// Clears the old viewport where the resized terminal now shows it and
/// builds a fresh one there, so its stale frame is neither left on screen
/// nor scrolled into scrollback, and the lines above it stay untouched.
fn rebuild(terminal: &mut Term, cursor_row: u16, height: u16) -> io::Result<Term> {
    let cursor = terminal.get_cursor_position()?;
    let top = cursor.y.saturating_sub(cursor_row);
    execute!(
        io::stdout(),
        MoveTo(0, top),
        Clear(ClearType::FromCursorDown)
    )?;
    inline_terminal(height)
}

/// crossterm's `EventStream` holds the input-reader lock while it waits, and
/// ratatui's inline `insert_before` needs that same lock to ask the terminal
/// for the cursor position — the query times out under a stream. Polling
/// with a short timeout on a thread releases the lock between polls.
fn spawn_input(stop: Arc<AtomicBool>) -> tokio::sync::mpsc::Receiver<io::Result<Input>> {
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let ev = match crossterm::event::poll(Duration::from_millis(50)) {
                Ok(true) => crossterm::event::read(),
                Ok(false) => continue,
                Err(e) => Err(e),
            };
            let failed = ev.is_err();
            if tx.blocking_send(ev).is_err() || failed {
                return;
            }
        }
    });
    rx
}

/// Leaves the terminal usable whatever happened; safe to call twice. `kitty`
/// pops the Kitty keyboard protocol flags first — popping when nothing was
/// pushed is a no-op on every terminal that implements the spec, but `run`
/// only pays for the round trip when its own push actually happened; `focus`
/// likewise turns off the focus reports only `run` turned on, `progress`
/// clears an OSC 9;4 state only a capable terminal was sent, and `mouse`
/// (T22.4) releases capture only when `tui.mouse` asked for it, so a `false`
/// config never touches the terminal's mouse reporting at all.
fn restore(kitty: bool, focus: bool, progress: bool, mouse: bool) {
    if progress {
        use std::io::Write;
        let mut out = io::stdout();
        let _ = out.write_all(crate::term::progress(crate::term::Progress::Idle).as_bytes());
        let _ = out.flush();
    }
    if kitty {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    if focus {
        let _ = execute!(io::stdout(), DisableFocusChange);
    }
    if mouse {
        let _ = execute!(io::stdout(), DisableMouseCapture);
    }
    let _ = execute!(io::stdout(), DisableBracketedPaste);
    let _ = disable_raw_mode();
}
