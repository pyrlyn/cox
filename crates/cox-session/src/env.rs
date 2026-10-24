// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Login-shell environment resolution (T37.11, DT§4.8). An app launched
//! from Finder or the Dock inherits `launchd`'s minimal environment, so
//! `bash` would not find `cargo` or `mise` and env-var API keys would be
//! missing. [`login_env`] asks the user's login shell for its environment
//! once, with a hard timeout, and falls back to the inherited one with a
//! [`Warning`]. Separate from session assembly because only the desktop
//! surface calls it; the CLI keeps the environment its terminal gave it.

use std::collections::BTreeMap;
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt as _;
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Stdio;
use std::time::Duration;

use crate::Warning;
#[cfg(unix)]
use cox_ext::proc::{TreeChild, WaitFail};

/// A resolved environment, sorted so callers and tests see a stable order.
pub type Env = BTreeMap<OsString, OsString>;

/// DT§4.8: an interactive zsh with a typical rc file took ~2 s on a loaded
/// machine, so 3 s fell back too often; 10 s (VS Code's cap) is only an upper
/// bound, a fast shell still answers at once.
pub const TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(target_os = "macos")]
const DEFAULT_SHELL: &str = "/bin/zsh";
#[cfg(all(unix, not(target_os = "macos")))]
const DEFAULT_SHELL: &str = "/bin/sh";

/// One spelling of each marker for both the script and the parser.
#[cfg(unix)]
macro_rules! marker {
    (begin) => {
        "__cox_env_begin_5d1c__"
    };
    (end) => {
        "__cox_env_end_5d1c__"
    };
}
#[cfg(unix)]
const BEGIN: &[u8] = marker!(begin).as_bytes();
#[cfg(unix)]
const END: &[u8] = marker!(end).as_bytes();

/// A constant script: nothing from the model, a repository or the user's
/// env is spliced into it. The markers fence `env -0` off from whatever
/// the rc files print on stdout.
#[cfg(unix)]
const SCRIPT: &str = concat!(
    "printf '%s' ",
    marker!(begin),
    "; env -0; printf '%s' ",
    marker!(end)
);

/// The environment of the user's login shell (`$SHELL`, else the platform
/// default). On any failure: the process environment and a warning.
///
/// Windows has no probe (T57.5, R10.8 unverified): the session keeps the
/// process environment. A login shell there is not something this crate has
/// observed.
pub async fn login_env(timeout: Duration) -> (Env, Option<Warning>) {
    #[cfg(unix)]
    {
        let shell = std::env::var_os("SHELL")
            .filter(|s| !s.is_empty())
            .map_or_else(|| PathBuf::from(DEFAULT_SHELL), PathBuf::from);
        resolve(&shell, timeout).await
    }
    #[cfg(not(unix))]
    {
        let _ = timeout;
        (std::env::vars_os().collect(), None)
    }
}

/// [`login_env`] with the shell named, so a test can hand it a fake one.
#[cfg(unix)]
pub async fn resolve(shell: &Path, timeout: Duration) -> (Env, Option<Warning>) {
    match capture(shell, timeout).await {
        Ok(env) => (env, None),
        Err(why) => (
            std::env::vars_os().collect(),
            Some(Warning::Env(format!(
                "login shell {}: {why}; using the inherited environment",
                shell.display()
            ))),
        ),
    }
}

#[cfg(unix)]
async fn capture(shell: &Path, timeout: Duration) -> Result<Env, String> {
    // `-i` as well as `-l` (DT§4.8): many users set PATH in `.zshrc`/`.bashrc`.
    // No stdin, so an interactive shell cannot block on a prompt.
    let mut cmd = tokio::process::Command::new(shell);
    cmd.args(["-l", "-i", "-c", SCRIPT])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = TreeChild::spawn(cmd).map_err(|e| format!("cannot start: {e}"))?;
    match child.wait_with_output(timeout).await {
        Ok(out) => parse(&out.stdout).ok_or_else(|| "printed no environment".to_string()),
        Err(WaitFail::Io(e)) => Err(format!("wait failed: {e}")),
        Err(WaitFail::TimedOut) => {
            // The helper already killed the group, so an rc file's children
            // are not still running or holding the pipe.
            Err(format!("timed out after {} ms", timeout.as_millis()))
        }
    }
}

/// Parses the fenced `env -0` output. Noise before the begin marker or
/// after the end marker is ignored; `None` if either marker is missing.
#[cfg(unix)]
pub fn parse(stdout: &[u8]) -> Option<Env> {
    let start = find(stdout, BEGIN)? + BEGIN.len();
    let body = &stdout[start..];
    let body = &body[..rfind(body, END)?];
    Some(
        body.split(|&b| b == 0)
            .filter_map(|entry| {
                let eq = entry.iter().position(|&b| b == b'=')?;
                (eq > 0).then(|| {
                    (
                        OsString::from_vec(entry[..eq].to_vec()),
                        OsString::from_vec(entry[eq + 1..].to_vec()),
                    )
                })
            })
            .collect(),
    )
}

#[cfg(unix)]
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(unix)]
fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    fn fake_shell(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("shell");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn get<'a>(env: &'a Env, key: &str) -> Option<&'a str> {
        env.get(&OsString::from(key)).and_then(|v| v.to_str())
    }

    #[tokio::test]
    async fn login_env_returns_the_shells_exported_path() {
        let dir = tempfile::tempdir().unwrap();
        // Runs the real script with `sh` after "rc files" print junk and
        // export PATH; args are `-l -i -c SCRIPT`, so the script is `$4`.
        let shell = fake_shell(
            dir.path(),
            "echo 'welcome back'\nexport PATH=/fake/login/bin:$PATH\nexec /bin/sh -c \"$4\"",
        );
        let (env, warning) = resolve(&shell, TIMEOUT).await;
        assert_eq!(warning, None);
        let path = get(&env, "PATH").unwrap();
        assert!(path.starts_with("/fake/login/bin:"), "{path}");
    }

    #[tokio::test]
    async fn a_shell_past_the_timeout_falls_back_to_the_process_env() {
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(dir.path(), "sleep 30");
        let started = std::time::Instant::now();
        let (env, warning) = resolve(&shell, Duration::from_millis(200)).await;
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(env, std::env::vars_os().collect::<Env>());
        assert!(matches!(warning, Some(Warning::Env(ref w)) if w.contains("timed out")));
    }

    #[tokio::test]
    async fn a_missing_shell_falls_back_with_a_warning() {
        let (env, warning) = resolve(Path::new("/nonexistent/cox-shell"), TIMEOUT).await;
        assert_eq!(env, std::env::vars_os().collect::<Env>());
        assert!(matches!(warning, Some(Warning::Env(_))));
    }

    #[test]
    fn parse_ignores_noise_around_the_markers() {
        let out =
            b"motd\n__cox_env_begin_5d1c__PATH=/a:/b\0MULTI=x\ny=z\0\0__cox_env_end_5d1c__bye\n";
        let env = parse(out).unwrap();
        assert_eq!(get(&env, "PATH"), Some("/a:/b"));
        assert_eq!(get(&env, "MULTI"), Some("x\ny=z"));
        assert_eq!(env.len(), 2);
    }

    #[test]
    fn parse_without_markers_is_none() {
        assert_eq!(parse(b"PATH=/a\0"), None);
        assert_eq!(parse(b"__cox_env_begin_5d1c__PATH=/a\0"), None);
    }

    fn alive(pid: i32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    async fn wait_dead(pid: i32) -> bool {
        for _ in 0..40 {
            if !alive(pid) {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        false
    }

    /// A shell that backgrounds `sleep` and then blocks. The timeout has to
    /// take the grandchild too, or the sleep keeps the session's process
    /// table busy after the probe has given up.
    #[tokio::test]
    async fn login_shell_timeout_kills_the_tree() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let shell = fake_shell(
            dir.path(),
            &format!("sleep 30 &\necho $! > {}\nwait\n", pidfile.display()),
        );
        let started = std::time::Instant::now();
        let (_env, warning) = resolve(&shell, std::time::Duration::from_secs(1)).await;
        assert!(started.elapsed() < std::time::Duration::from_secs(8));
        assert!(matches!(warning, Some(Warning::Env(ref w)) if w.contains("timed out")));
        let pid: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(
            wait_dead(pid).await,
            "backgrounded sleep {pid} survived the login-shell timeout"
        );
    }
}
