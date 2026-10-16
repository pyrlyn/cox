// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The launch command for a target (T68.1): the launcher's argv per kind,
//! with the environment scrubbed to a short list. Separate from `lib` so the
//! argv shapes, which come from each tool's own docs, sit with the tests that
//! pin them. It builds a `std::process::Command` and starts nothing, so this
//! crate needs no async runtime; callers convert with
//! `tokio::process::Command::from` and own stdio and `kill_on_drop`.

use std::path::Path;
use std::process::Command;

use crate::{Kind, Target, TargetError, check_name, check_token};

/// Whether the remote command gets a terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tty {
    /// A pipe: the app-server protocol, or a script.
    No,
    /// A pseudo-terminal: an interactive shell in the target.
    Yes,
}

#[cfg(not(windows))]
macro_rules! env_list {
    ($($extra:literal),*) => {
        &["HOME", "USER", "LOGNAME", "PATH", "LANG", "TERM", "COLORTERM" $(, $extra)*]
    };
}

#[cfg(windows)]
macro_rules! env_list {
    ($($extra:literal),*) => {
        &[
            "SystemRoot", "USERPROFILE", "USERNAME", "APPDATA", "LOCALAPPDATA", "PATH",
            "PATHEXT", "TERM" $(, $extra)*
        ]
    };
}

/// The variables a launcher may still see, per kind and OS. `PATH` stays
/// because colima finds `limactl` and docker finds its credential helpers
/// through it; the launcher itself was not found through `PATH`.
pub fn kept_env(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Ssh => env_list!("SSH_AUTH_SOCK"),
        Kind::Docker => env_list!(
            "DOCKER_HOST",
            "DOCKER_CONTEXT",
            "DOCKER_CONFIG",
            "DOCKER_CERT_PATH",
            "DOCKER_TLS_VERIFY"
        ),
        Kind::Lima => env_list!("LIMA_HOME"),
        Kind::Colima => env_list!("COLIMA_HOME", "LIMA_HOME"),
    }
}

/// `launcher` running `remote` inside `target`, not yet started.
///
/// Names are re-checked here, not only when parsed, because a `Target` can
/// also be built by hand; every `remote` token is checked because three of
/// the launchers join them into a shell string.
pub fn command(
    target: &Target,
    launcher: &Path,
    remote: &[&str],
    tty: Tty,
) -> Result<Command, TargetError> {
    check_name(target.name())?;
    remote.iter().try_for_each(|t| check_token(t))?;
    let mut command = Command::new(launcher);
    match target {
        Target::Ssh { host } => {
            command
                .arg(if tty == Tty::Yes { "-t" } else { "-T" })
                .args(["-o", "BatchMode=yes", "-o", "ForwardAgent=no"])
                .args(["-o", "ClearAllForwardings=yes", "--", host]);
        }
        Target::Docker { container, context } => {
            // `--context` is docker's global flag, so it precedes `exec`.
            if let Some(context) = context {
                check_name(context)?;
                command.args(["--context", context]);
            }
            command.args(["exec", "-i"]);
            if tty == Tty::Yes {
                command.arg("-t");
            }
            command.arg(container);
        }
        Target::Lima { instance } => {
            command.arg("shell");
            if tty == Tty::No {
                // `--tty` is limactl's global flag: "Set to false for automation".
                command.arg("--tty=false");
            }
            command.arg(instance);
        }
        Target::Colima { profile } => {
            // `--profile` is colima's persistent root flag; `ssh` passes what
            // follows `--` through to the VM.
            command.args(["ssh", "--profile", profile, "--"]);
        }
    }
    command.args(remote);
    command.env_clear().envs(
        kept_env(target.kind())
            .iter()
            .filter_map(|k| Some((*k, std::env::var_os(k)?))),
    );
    Ok(command)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn shape(target: &Target, tty: Tty) -> String {
        let launcher = PathBuf::from("/launch/").join(target.kind().program());
        let command = command(target, &launcher, &["cox", "app-server", "--stdio"], tty)
            .expect("a valid target");
        let mut line = command.get_program().to_string_lossy().into_owned();
        for arg in command.get_args() {
            line.push(' ');
            line.push_str(&arg.to_string_lossy());
        }
        line
    }

    #[test]
    fn each_kind_builds_its_launch_argv() {
        let targets = [
            "ssh:devbox".parse::<Target>(),
            "docker:web".parse(),
            "lima:default".parse(),
            "colima:dev".parse(),
        ]
        .map(|t| t.expect("a valid target"));
        let with_context = Target::Docker {
            container: "web".into(),
            context: Some("remote".into()),
        };
        let mut out = String::new();
        for target in targets.iter().chain([&with_context]) {
            for tty in [Tty::No, Tty::Yes] {
                out.push_str(&format!("{target} {tty:?}: {}\n", shape(target, tty)));
            }
        }
        insta::assert_snapshot!(out);
    }

    #[test]
    fn name_that_reads_as_an_option_never_reaches_a_command() {
        for target in [
            Target::Ssh {
                host: "-p22".into(),
            },
            Target::Docker {
                container: "-v".into(),
                context: None,
            },
            Target::Docker {
                container: "web".into(),
                context: Some("-H".into()),
            },
            Target::Lima {
                instance: "dev box".into(),
            },
            Target::Colima {
                profile: String::new(),
            },
        ] {
            assert!(
                command(&target, Path::new("/x"), &["cox"], Tty::No).is_err(),
                "{target:?} was accepted"
            );
        }
        let ssh = Target::Ssh {
            host: "devbox".into(),
        };
        assert!(command(&ssh, Path::new("/x"), &["a b"], Tty::No).is_err());
    }

    #[test]
    #[allow(unsafe_code)]
    fn launch_keeps_only_the_allowed_env() {
        // why: env::set_var is unsafe in edition 2024; a key in this process
        // must not follow the session to the target.
        // SAFETY: nextest runs each test in its own process.
        unsafe { std::env::set_var("ANTHROPIC_API_KEY", "sk-must-not-leave") };
        for text in ["ssh:devbox", "docker:web", "lima:default", "colima:dev"] {
            let target: Target = text.parse().expect("valid");
            let command = command(&target, Path::new("/x"), &["cox"], Tty::No).expect("valid");
            let allowed = kept_env(target.kind());
            for (key, _) in command.get_envs() {
                let key = key.to_string_lossy();
                assert!(allowed.contains(&&*key), "{key} reaches {text}");
            }
            assert!(command.get_envs().all(|(k, _)| k != "ANTHROPIC_API_KEY"));
        }
    }
}
