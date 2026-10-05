// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Append-only JSONL rollout: one line per `Event`,
//! `{"seq":N,"ts":"…","event":{…}}` (plan.md §1.7). `seq` is monotonic per
//! session; writes fsync every `ROLLOUT_FSYNC_EVERY` lines and on
//! `Event::TurnDone`; reads tolerate one truncated (unparseable) last line,
//! the shape a crash mid-write leaves behind.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;

use cox_protocol::Event;
use serde::{Deserialize, Serialize};

use crate::ROLLOUT_FSYNC_EVERY;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RolloutLine {
    seq: u64,
    ts: String,
    event: Event,
}

/// An open rollout file plus enough state to append without rescanning it
/// on every call.
pub(crate) struct RolloutWriter {
    file: File,
    next_seq: u64,
    unsynced: u32,
}

impl RolloutWriter {
    /// Opens (creating if needed) the rollout file at `path`, resuming
    /// `next_seq` from the count of lines it could already parse, so `seq`
    /// stays monotonic across process restarts.
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let (events, _truncated) = read_lines(path)?;
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            file,
            next_seq: events.len() as u64 + 1,
            unsynced: 0,
        })
    }

    /// Appends one event, returning its sequence number.
    pub(crate) fn append(&mut self, ts: String, event: &Event) -> io::Result<u64> {
        let seq = self.next_seq;
        let line = RolloutLine {
            seq,
            ts,
            event: event.clone(),
        };
        let json = serde_json::to_string(&line)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        writeln!(self.file, "{json}")?;
        self.next_seq += 1;
        self.unsynced += 1;

        // ponytail: TurnDone always forces a sync (turn boundaries must be
        // durable); everything else batches up to ROLLOUT_FSYNC_EVERY lines.
        // Upgrade to per-item-kind sync policy if a narrower loss window
        // than "up to 16 events" is ever needed.
        let force_sync = matches!(event, Event::TurnDone { .. });
        if force_sync || self.unsynced >= ROLLOUT_FSYNC_EVERY {
            self.file.sync_data()?;
            self.unsynced = 0;
        }
        Ok(seq)
    }
}

/// Reads back every event in a rollout file, tolerating one truncated
/// (unparseable) final line. Returns `(events, true)` when the last line
/// was dropped as truncated; a missing file reads as empty, not an error.
pub(crate) fn read_lines(path: &Path) -> io::Result<(Vec<Event>, bool)> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((Vec::new(), false)),
        Err(e) => return Err(e),
    };
    let raw_lines: Vec<String> = BufReader::new(file).lines().collect::<io::Result<_>>()?;

    let mut events = Vec::with_capacity(raw_lines.len());
    let mut truncated = false;
    for (i, raw) in raw_lines.iter().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<RolloutLine>(raw) {
            Ok(parsed) => events.push(parsed.event),
            Err(_) if i + 1 == raw_lines.len() => truncated = true,
            Err(e) => return Err(io::Error::new(io::ErrorKind::InvalidData, e)),
        }
    }
    Ok((events, truncated))
}

#[cfg(test)]
mod tests {
    use cox_protocol::{StopReason, TurnId};

    use super::*;

    fn turn_done() -> Event {
        Event::TurnDone {
            turn: TurnId::new(),
            stop: StopReason::EndTurn,
        }
    }

    #[test]
    fn append_and_read_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        let mut w = RolloutWriter::open(&path).expect("open");
        w.append("2026-09-02T00:00:00.000Z".into(), &turn_done())
            .expect("append");
        drop(w);

        let (events, truncated) = read_lines(&path).expect("read");
        assert_eq!(events.len(), 1);
        assert!(!truncated);
    }

    #[test]
    fn writer_resumes_seq_after_reopen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        let mut w = RolloutWriter::open(&path).expect("open");
        for _ in 0..3 {
            w.append("2026-09-02T00:00:00.000Z".into(), &turn_done())
                .expect("append");
        }
        drop(w);

        let mut w2 = RolloutWriter::open(&path).expect("reopen");
        let seq = w2
            .append("2026-09-02T00:00:01.000Z".into(), &turn_done())
            .expect("append");
        assert_eq!(seq, 4);
    }

    #[test]
    fn truncated_last_line_is_dropped_not_fatal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        let mut w = RolloutWriter::open(&path).expect("open");
        w.append("2026-09-02T00:00:00.000Z".into(), &turn_done())
            .expect("append");
        drop(w);

        // Simulate a crash mid-write: an incomplete JSON line with no
        // trailing newline, appended directly to the file.
        let mut f = OpenOptions::new().append(true).open(&path).expect("open");
        write!(f, "{{\"seq\":2,\"ts\":\"2026\",\"event\":{{\"typ").expect("write partial");
        drop(f);

        let (events, truncated) = read_lines(&path).expect("read");
        assert_eq!(events.len(), 1);
        assert!(truncated);
    }
}
