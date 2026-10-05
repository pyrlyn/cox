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
use std::os::unix::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use crate::Warning;

/// A resolved environment, sorted so callers and tests see a stable order.
pub type Env = BTreeMap<OsString, OsString>;

/// DT§4.8: an interactive zsh with a typical rc file took ~2 s on a loaded
/// machine, so 3 s fell back too often; 10 s (VS Code's cap) is only an upper
/// bound, a fast shell still answers at once.
pub const TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(target_os = "macos")]
const DEFAULT_SHELL: &str = "/bin/zsh";
#[cfg(not(target_os = "macos"))]
const DEFAULT_SHELL: &str = "/bin/sh";

/// One spelling of each marker for both the script and the parser.
macro_rules! marker {
    (begin) => {
        "__cox_env_begin_5d1c__"
    };
    (end) => {
        "__cox_env_end_5d1c__"
    };
}
const BEGIN: &[u8] = marker!(begin).as_bytes();
const END: &[u8] = marker!(end).as_bytes();

/// A constant script: nothing from the model, a repository or the user's
/// env is spliced into it. The markers fence `env -0` off from whatever
/// the rc files print on stdout.
const SCRIPT: &str = concat!(
    "printf '%s' ",
    marker!(begin),
    "; env -0; printf '%s' ",
    marker!(end)
);

/// The environment of the user's login shell (`$SHELL`, else the platform
/// default). On any failure: the process environment and a warning.
pub async fn login_env(timeout: Duration) -> (Env, Option<Warning>) {
    let shell = std::env::var_os("SHELL")
        .filter(|s| !s.is_empty())
        .map_or_else(|| PathBuf::from(DEFAULT_SHELL), PathBuf::from);
    resolve(&shell, timeout).await
}

/// [`login_env`] with the shell named, so a test can hand it a fake one.
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

async fn capture(shell: &Path, timeout: Duration) -> Result<Env, String> {
    // `-i` as well as `-l` (DT§4.8): many users set PATH in `.zshrc`/`.bashrc`.
    // No stdin, so an interactive shell cannot block on a prompt.
    let mut cmd = tokio::process::Command::new(shell);
    cmd.args(["-l", "-i", "-c", SCRIPT])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .kill_on_drop(true);
    let child = cmd.spawn().map_err(|e| format!("cannot start: {e}"))?;
    let pid = child.id();
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(out)) => parse(&out.stdout).ok_or_else(|| "printed no environment".to_string()),
        Ok(Err(e)) => Err(format!("wait failed: {e}")),
        Err(_) => {
            // `kill_on_drop` took the shell; the group takes whatever an rc
            // file started, so nothing keeps running or holds the pipe.
            if let Some(pid) = pid.and_then(|p| i32::try_from(p).ok()) {
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(pid),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
            Err(format!("timed out after {} ms", timeout.as_millis()))
        }
    }
}

/// Parses the fenced `env -0` output. Noise before the begin marker or
/// after the end marker is ignored; `None` if either marker is missing.
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

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

#[cfg(test)]
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
}
