// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! One kill path for a child and everything it starts (T57.5).
//!
//! A hook and the login-shell probe both need a timeout that ends the whole
//! tree. Unix puts the child in its own process group so `start_kill` is
//! `killpg(SIGKILL)`. Windows puts it in a job object so `start_kill` is
//! `TerminateJobObject`. Callers share this instead of each keeping a
//! `killpg`. T57.6's remaining spawns use the same helper.

use std::process::Output;
use std::time::Duration;

use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{ChildStdin, Command};

/// Why [`TreeChild::wait_with_output`] did not return the child's output.
pub enum WaitFail {
    /// The deadline passed. The tree has already been killed.
    TimedOut,
    /// Spawn already succeeded; waiting or reading the pipes failed.
    Io(std::io::Error),
}

/// A child whose timeout kills its descendants too.
pub struct TreeChild {
    inner: Box<dyn ChildWrapper>,
}

impl TreeChild {
    /// Spawn `command` as a group leader (Unix) or inside a job (Windows).
    /// Dropping the child still kills it (`KillOnDrop`); a timeout goes
    /// through [`Self::wait_with_output`], which kills the whole tree first.
    pub fn spawn(command: Command) -> std::io::Result<Self> {
        let mut wrap = CommandWrap::from(command);
        // The group or job wrapper has to be the child wrapper `start_kill`
        // reaches. `KillOnDrop` only sets the flag other wrappers read; it
        // does not wrap the child.
        #[cfg(unix)]
        wrap.wrap(process_wrap::tokio::ProcessGroup::leader());
        #[cfg(windows)]
        wrap.wrap(process_wrap::tokio::JobObject);
        wrap.wrap(KillOnDrop);
        Ok(Self {
            inner: wrap.spawn()?,
        })
    }

    /// The child's stdin, if the command asked for a pipe.
    pub fn stdin(&mut self) -> &mut Option<ChildStdin> {
        self.inner.stdin()
    }

    /// Wait until `limit`, reading both pipes while the child runs.
    ///
    /// Reading after `wait` deadlocks once a pipe fills, so the reads run
    /// alongside the wait. On the deadline, `start_kill` runs while this
    /// value is still held: `wait_with_output` on the wrapper would have
    /// consumed it, and the dropped wait would only kill the leader.
    pub async fn wait_with_output(&mut self, limit: Duration) -> Result<Output, WaitFail> {
        let stdout = self.inner.stdout().take();
        let stderr = self.inner.stderr().take();
        let waited = tokio::time::timeout(limit, async {
            let (status, stdout, stderr) =
                tokio::try_join!(self.inner.wait(), read_pipe(stdout), read_pipe(stderr),)?;
            Ok(Output {
                status,
                stdout,
                stderr,
            })
        })
        .await;
        match waited {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(error)) => Err(WaitFail::Io(error)),
            Err(_elapsed) => {
                let _ = self.inner.start_kill();
                let _ = self.inner.wait().await;
                Err(WaitFail::TimedOut)
            }
        }
    }
}

async fn read_pipe(mut pipe: Option<impl AsyncRead + Unpin>) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    if let Some(pipe) = pipe.as_mut() {
        pipe.read_to_end(&mut buf).await?;
    }
    Ok(buf)
}
