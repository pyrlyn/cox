// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox attach --stdio` (T66.22, `docs/design/serve.md` D4.6): copies bytes
//! between stdio and the supervisor's socket, so `ssh <host> cox attach
//! --stdio` reaches resident sessions with ssh's own authentication deciding
//! who gets in, and cox opens no inbound port. Separate from `router` because
//! it is the client's end: it reads no line, and the router decodes and
//! refuses each one as it would any other connection's.

use std::path::{Path, PathBuf};

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::UnixStream;

use crate::socket;

/// Why the bridge stopped.
#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    /// No supervisor listens there.
    #[error("cox serve is not reachable at {path}: {source}")]
    Connect {
        /// The socket path.
        path: PathBuf,
        /// The OS error.
        source: std::io::Error,
    },
    /// The socket's server runs as another user.
    #[error("{path} is served by another user; refusing to attach")]
    Foreign {
        /// The socket path.
        path: PathBuf,
    },
    /// Copying failed.
    #[error("the bridge: {0}")]
    Io(#[from] std::io::Error),
}

/// Bridges this process's stdin and stdout to `home`'s (`COX_HOME`)
/// supervisor until both directions end.
pub async fn stdio(home: &Path) -> Result<(), AttachError> {
    let mut stdio = tokio::io::join(tokio::io::stdin(), tokio::io::stdout());
    bridge(&socket::path(home), &mut stdio).await
}

/// Bridges `client` to the socket at `path`.
pub async fn bridge<S>(path: &Path, client: &mut S) -> Result<(), AttachError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut server = UnixStream::connect(path)
        .await
        .map_err(|source| AttachError::Connect {
            path: path.to_path_buf(),
            source,
        })?;
    // The same uid check the supervisor makes, from this side: a socket
    // someone else put in place never gets this user's lines.
    if !socket::admit_peer(&server, socket::own_uid()) {
        return Err(AttachError::Foreign {
            path: path.to_path_buf(),
        });
    }
    tokio::io::copy_bidirectional(client, &mut server).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;
    use tokio::time::timeout;

    const WAIT: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn the_bridge_copies_both_ways() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.sock");
        let listener = UnixListener::bind(&path).expect("bind");
        let echo = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            while let Some(line) = lines.next_line().await.expect("read") {
                write
                    .write_all(format!("{line}!\n").as_bytes())
                    .await
                    .expect("echo");
            }
        });
        let (mut ours, mut theirs) = tokio::io::duplex(1024);
        let bridged = tokio::spawn(async move { bridge(&path, &mut theirs).await });
        ours.write_all(b"hello\n").await.expect("write");
        ours.shutdown().await.expect("shutdown");
        let mut echoed = String::new();
        let read = ours.read_to_string(&mut echoed);
        timeout(WAIT, read).await.expect("in time").expect("read");
        assert_eq!(echoed, "hello!\n");
        let result = timeout(WAIT, bridged).await.expect("in time");
        assert!(result.expect("joined").is_ok());
        echo.await.expect("echo");
    }

    #[tokio::test]
    async fn no_supervisor_names_the_socket() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = socket::path(dir.path());
        let (_, mut theirs) = tokio::io::duplex(64);
        let error = bridge(&path, &mut theirs)
            .await
            .expect_err("nothing listens");
        assert!(error.to_string().contains("serve.sock"), "{error}");
    }
}
