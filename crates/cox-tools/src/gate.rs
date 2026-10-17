// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `ShellGateRunner` (T66.11): runs an autonomous run's gate commands. It
//! lives beside `bash` because a gate is a shell command and must meet the
//! same trust boundary: no spawn path of its own, every gate goes through
//! `bash::run_line` (the platform sandbox wrap, the env allowlist, the PTY
//! and the process-group kill). `cox-core` only decides on the result
//! (`cox_core::autonomous`), so it never starts a process.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cox_protocol::autonomous::{GateResult, GateRunner};
use cox_protocol::{Archive, ArchivePut, CallId, SandboxPolicy, SessionId};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::bash::run_line;

/// Lines of output a failed gate hands back to the model.
const TAIL_LINES: usize = 80;
/// Bytes of that tail. The model sees a pointer to the rest, not the rest.
const TAIL_BYTES: usize = 8 * 1024;
/// Output kept in memory and archived per gate: a gate that prints without
/// end cannot grow cox's memory, and what is lost is the oldest output.
const MAX_CAPTURE: usize = 1 << 20;

/// Runs gate commands as `bash` would, in the session's sandbox.
pub struct ShellGateRunner {
    cwd: PathBuf,
    roots: Vec<PathBuf>,
    sandbox: SandboxPolicy,
    archive: Arc<dyn Archive>,
    session: SessionId,
    cancel: CancellationToken,
}

impl ShellGateRunner {
    /// A runner for `session`. `roots` are the workspace roots the sandbox
    /// opens and `archive` is where a gate's full output lands before the
    /// model sees its tail.
    pub fn new(
        cwd: PathBuf,
        roots: Vec<PathBuf>,
        sandbox: SandboxPolicy,
        archive: Arc<dyn Archive>,
        session: SessionId,
    ) -> Self {
        Self {
            cwd,
            roots,
            sandbox,
            archive,
            session,
            cancel: CancellationToken::new(),
        }
    }

    /// Ends the gate being run and fails every later one at once. The
    /// process group is signalled, so nothing the gate started survives.
    /// Kept as a method so the caller needs no `CancellationToken` of its own.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Whether `cancel` was called.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// The model-facing end of `output`. The whole of it is archived first
    /// (D6a); if the archive fails the tail still returns, saying so, since a
    /// failed gate must reach the driver either way.
    async fn tail(&self, command: &str, output: &str, dropped: bool) -> String {
        let (tail, cut) = cap(output);
        if !cut && !dropped {
            return tail;
        }
        let put = ArchivePut {
            session: self.session,
            call: CallId::new(),
            tool: "autonomous_gate".into(),
            subject: Some(command.to_owned()),
            bytes: output.as_bytes().to_vec(),
        };
        match self.archive.put(put).await {
            Ok(id) => format!("[… earlier output archived; expand #{id}]\n{tail}"),
            Err(_) => format!("[… earlier output dropped; it could not be archived]\n{tail}"),
        }
    }
}

#[async_trait]
impl GateRunner for ShellGateRunner {
    async fn run(&self, command: &str, timeout: Duration) -> GateResult {
        let (tx, mut rx) = mpsc::channel::<String>(64);
        let runner = async {
            let exit = run_line(
                command,
                &self.cwd,
                &self.roots,
                &self.sandbox,
                &self.cancel,
                &tx,
                timeout,
            )
            .await;
            drop(tx);
            exit
        };
        let drain = async {
            let mut kept = String::new();
            let mut dropped = false;
            while let Some(chunk) = rx.recv().await {
                kept.push_str(&chunk);
                let over = kept.len().saturating_sub(MAX_CAPTURE);
                if over > 0 {
                    let cut = (over..=kept.len())
                        .find(|&i| kept.is_char_boundary(i))
                        .unwrap_or(kept.len());
                    kept.drain(..cut);
                    dropped = true;
                }
            }
            (kept, dropped)
        };
        let (exit, (mut output, dropped)) = tokio::join!(runner, drain);
        let (passed, note) = match exit {
            Ok(exit) => match (exit.stopped, exit.code) {
                (None, Some(0)) => (true, None),
                (Some(why), _) => (false, Some(format!("[gate {why}]"))),
                (None, Some(code)) => (false, Some(format!("[exit {code}]"))),
                (None, None) => (false, Some("[exit unknown]".to_owned())),
            },
            // A spawn failure is a failed gate, not a crashed run: the
            // driver's retry limit bounds it like any other failure.
            Err(e) => (false, Some(format!("[gate could not start: {e}]"))),
        };
        if let Some(note) = note {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&note);
        }
        let tail = if passed {
            String::new()
        } else {
            self.tail(command, &output, dropped).await
        };
        GateResult {
            command: command.to_owned(),
            passed,
            tail,
        }
    }
}

/// The last `TAIL_LINES` lines of `text`, cut at a line start to fit
/// `TAIL_BYTES`; `true` when anything was cut. A single line over the cap is
/// cut on a character boundary instead.
fn cap(text: &str) -> (String, bool) {
    let text = text.trim_end_matches('\n');
    let lines: Vec<&str> = text.lines().collect();
    let mut start = lines.len().saturating_sub(TAIL_LINES);
    let size = |from: usize| lines[from..].iter().map(|l| l.len() + 1).sum::<usize>();
    while start < lines.len().saturating_sub(1) && size(start) > TAIL_BYTES {
        start += 1;
    }
    let joined = lines[start..].join("\n");
    if joined.len() <= TAIL_BYTES {
        return (joined, start > 0);
    }
    let from = (joined.len() - TAIL_BYTES..=joined.len())
        .find(|&i| joined.is_char_boundary(i))
        .unwrap_or(joined.len());
    (joined[from..].to_owned(), true)
}

#[cfg(all(test, unix))]
mod tests {
    use std::sync::Mutex;

    use cox_protocol::{ArchiveId, SandboxMode, StoreError};

    use super::*;

    #[derive(Default)]
    struct Recorded(Mutex<Vec<ArchivePut>>);

    #[async_trait]
    impl Archive for Recorded {
        async fn put(&self, put: ArchivePut) -> Result<ArchiveId, StoreError> {
            self.0.lock().expect("lock").push(put);
            Ok(ArchiveId::new())
        }
        async fn get(&self, _id: &ArchiveId) -> Result<Vec<u8>, StoreError> {
            Ok(Vec::new())
        }
    }

    /// `DangerFullAccess` only so the unit tests do not depend on the host's
    /// sandbox backend; the e2e test in `crates/cox` runs the real wrap.
    fn runner(dir: &std::path::Path, archive: Arc<Recorded>) -> ShellGateRunner {
        let sandbox = SandboxPolicy {
            mode: SandboxMode::DangerFullAccess,
            network: true,
            writable: vec![],
            readonly_in_workspace: vec![],
            linux_backend: Default::default(),
        };
        ShellGateRunner::new(
            dir.to_path_buf(),
            vec![dir.to_path_buf()],
            sandbox,
            archive,
            SessionId::new(),
        )
    }

    const SECS: Duration = Duration::from_secs(20);

    #[tokio::test]
    async fn a_gate_that_exits_zero_passes_with_no_tail() {
        let (dir, archive) = (
            tempfile::tempdir().expect("dir"),
            Arc::new(Recorded::default()),
        );
        let result = runner(dir.path(), archive.clone())
            .run("echo fine", SECS)
            .await;
        assert!(result.passed);
        assert_eq!(result.tail, "");
        assert!(archive.0.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn a_failing_gate_reports_its_output_and_exit_code() {
        let (dir, archive) = (
            tempfile::tempdir().expect("dir"),
            Arc::new(Recorded::default()),
        );
        let result = runner(dir.path(), archive)
            .run("echo 1 test failed; exit 3", SECS)
            .await;
        assert!(!result.passed);
        assert_eq!(result.tail, "1 test failed\n[exit 3]");
    }

    #[tokio::test]
    async fn a_gate_over_its_timeout_fails_and_says_so() {
        let (dir, archive) = (
            tempfile::tempdir().expect("dir"),
            Arc::new(Recorded::default()),
        );
        let result = runner(dir.path(), archive)
            .run("sleep 30", Duration::from_millis(300))
            .await;
        assert!(!result.passed);
        assert!(result.tail.contains("[gate timed out]"), "{}", result.tail);
    }

    #[tokio::test]
    async fn a_cancelled_runner_fails_its_gate_without_waiting_for_it() {
        let (dir, archive) = (
            tempfile::tempdir().expect("dir"),
            Arc::new(Recorded::default()),
        );
        let runner = runner(dir.path(), archive);
        runner.cancel();
        let result = runner.run("sleep 30", SECS).await;
        assert!(!result.passed && runner.is_cancelled());
        assert!(result.tail.contains("[gate cancelled]"), "{}", result.tail);
    }

    #[tokio::test]
    async fn long_output_is_archived_whole_before_its_tail_is_returned() {
        let (dir, archive) = (
            tempfile::tempdir().expect("dir"),
            Arc::new(Recorded::default()),
        );
        let result = runner(dir.path(), archive.clone())
            .run("seq 1 500; exit 1", SECS)
            .await;
        assert!(!result.passed);
        assert!(
            result
                .tail
                .starts_with("[… earlier output archived; expand #")
        );
        assert!(result.tail.contains("\n500\n[exit 1]"), "{}", result.tail);
        assert!(!result.tail.contains("\n1\n"));
        let puts = archive.0.lock().expect("lock");
        let [put] = puts.as_slice() else {
            panic!("one archive row, got {}", puts.len());
        };
        let text = String::from_utf8(put.bytes.clone()).expect("utf8");
        assert!(text.starts_with("1\n2\n") && text.ends_with("[exit 1]"));
        assert_eq!(put.subject.as_deref(), Some("seq 1 500; exit 1"));
    }

    #[test]
    fn cap_cuts_one_huge_line_on_a_character_boundary() {
        let text = "é".repeat(TAIL_BYTES);
        let (tail, cut) = cap(&text);
        assert!(cut);
        assert!(tail.len() <= TAIL_BYTES && tail.chars().all(|c| c == 'é'));
    }
}
