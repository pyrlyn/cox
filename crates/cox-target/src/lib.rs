// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Run targets (T68.1, DT§4.4): the machine or container a session runs on
//! (`ssh:devbox`, `docker:web`, `lima:default`, `colima:dev`), its textual
//! form, and the checks on every string that becomes an argument of the
//! launcher program. Separate from `cox-app` because the desktop link, the
//! `--on` flag and the agent presets all build these commands, and the argv
//! rules are a trust guard that must have one owner. Pure: no workspace
//! dependency, no I/O; `command` only describes a process.

use std::fmt;
use std::str::FromStr;

pub mod command;
pub mod launcher;

pub use command::{Tty, command, kept_env};
pub use launcher::{candidates, find, launcher};

/// What went wrong choosing or launching a target.
#[derive(Debug, thiserror::Error)]
pub enum TargetError {
    /// A name that a launcher could read as an option or split into words.
    #[error("{0:?} is not a target name")]
    BadName(String),
    /// The text before the first `:` names no known kind.
    #[error("{0:?} is not a target kind; use ssh, docker, lima or colima")]
    UnknownKind(String),
    /// A remote argument the launcher's remote shell would reinterpret.
    #[error("{0:?} cannot be passed to a remote command")]
    BadToken(String),
    /// No launcher program at any of the fixed paths.
    #[error("{0} not found; set its path in config")]
    NoLauncher(&'static str),
}

/// How a target is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A host over `ssh`.
    Ssh,
    /// A running container through `docker exec`.
    Docker,
    /// A Lima instance through `limactl shell`.
    Lima,
    /// A Colima profile through `colima ssh`.
    Colima,
}

impl Kind {
    /// The launcher program's name.
    pub fn program(self) -> &'static str {
        match self {
            Self::Ssh => "ssh",
            Self::Docker => "docker",
            Self::Lima => "limactl",
            Self::Colima => "colima",
        }
    }

    /// The word before the `:` in a target's text.
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Ssh => "ssh",
            Self::Docker => "docker",
            Self::Lima => "lima",
            Self::Colima => "colima",
        }
    }
}

/// Where a session runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Target {
    /// `ssh <host>`; the host as ssh reads it (`me@box`, an alias).
    Ssh {
        /// Host name or alias.
        host: String,
    },
    /// `docker exec <container>`.
    Docker {
        /// Container name or id.
        container: String,
        /// `docker --context`; set from config, never parsed from text.
        context: Option<String>,
    },
    /// `limactl shell <instance>`.
    Lima {
        /// Instance name.
        instance: String,
    },
    /// `colima ssh --profile <profile>`.
    Colima {
        /// Profile name.
        profile: String,
    },
}

impl Target {
    /// How this target is reached.
    pub fn kind(&self) -> Kind {
        match self {
            Self::Ssh { .. } => Kind::Ssh,
            Self::Docker { .. } => Kind::Docker,
            Self::Lima { .. } => Kind::Lima,
            Self::Colima { .. } => Kind::Colima,
        }
    }

    /// The host, container, instance or profile.
    pub fn name(&self) -> &str {
        match self {
            Self::Ssh { host } => host,
            Self::Docker { container, .. } => container,
            Self::Lima { instance } => instance,
            Self::Colima { profile } => profile,
        }
    }
}

impl FromStr for Target {
    type Err = TargetError;

    /// `kind:name`, split at the first `:` only, so `ssh:devbox:2222` and
    /// `ssh:[::1]` keep their colons in the name.
    fn from_str(s: &str) -> Result<Self, TargetError> {
        let (kind, name) = s
            .split_once(':')
            .ok_or_else(|| TargetError::UnknownKind(s.to_owned()))?;
        check_name(name)?;
        let name = name.to_owned();
        match kind {
            "ssh" => Ok(Self::Ssh { host: name }),
            "docker" => Ok(Self::Docker {
                container: name,
                context: None,
            }),
            "lima" => Ok(Self::Lima { instance: name }),
            "colima" => Ok(Self::Colima { profile: name }),
            other => Err(TargetError::UnknownKind(other.to_owned())),
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind().prefix(), self.name())
    }
}

/// Refuses a name a launcher could take for an option (`-oProxyCommand=…`)
/// or split into several words. A name is local argv and never reaches a
/// remote shell, so it is stricter than nothing and looser than a token.
pub fn check_name(name: &str) -> Result<(), TargetError> {
    let bad = name.is_empty()
        || name.starts_with('-')
        || name.chars().any(|c| c.is_whitespace() || c.is_control());
    if bad {
        return Err(TargetError::BadName(name.to_owned()));
    }
    Ok(())
}

/// Refuses a remote argument unless every character is plainly safe.
/// `ssh`, `limactl shell` and `colima ssh` join their argv into one string
/// for the remote shell, so a quote or `;` in a token would be parsed there;
/// an allow-list is the only rule that stays right for shells nobody
/// listed. `\` is allowed for Windows paths.
pub fn check_token(token: &str) -> Result<(), TargetError> {
    let ok = !token.is_empty()
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.,/:=@+\\".contains(c));
    if ok {
        Ok(())
    } else {
        Err(TargetError::BadToken(token.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_round_trips_through_its_string() {
        for text in [
            "ssh:devbox",
            "ssh:devbox:2222",
            "ssh:me@devbox.local",
            "ssh:[::1]",
            "docker:web-1",
            "lima:default",
            "colima:dev",
        ] {
            let target: Target = text.parse().expect("a valid target");
            assert_eq!(target.to_string(), text);
        }
        let target: Target = "ssh:devbox:2222".parse().expect("valid");
        assert_eq!(target.kind(), Kind::Ssh);
        assert_eq!(target.name(), "devbox:2222");
    }

    #[test]
    fn unknown_kind_is_refused() {
        for text in ["", "devbox", "podman:x", ":x", "SSH:x"] {
            assert!(
                matches!(text.parse::<Target>(), Err(TargetError::UnknownKind(_))),
                "{text:?} was accepted"
            );
        }
    }

    #[test]
    fn name_that_reads_as_an_option_is_refused() {
        for kind in ["ssh", "docker", "lima", "colima"] {
            for name in [
                "-oProxyCommand=touch /tmp/x",
                "-p22",
                "",
                "dev box",
                "dev\nbox",
                "a\u{7}",
            ] {
                let text = format!("{kind}:{name}");
                assert!(
                    matches!(text.parse::<Target>(), Err(TargetError::BadName(_))),
                    "{text:?} was accepted"
                );
            }
        }
        assert!(check_name("dev box").is_err());
        assert!(check_name("devbox").is_ok());
    }

    #[test]
    fn token_with_shell_metacharacters_is_refused() {
        for token in [
            "",
            "a b",
            "a'b",
            "a\"b",
            "a`b",
            "$HOME",
            "a;b",
            "a&b",
            "a|b",
            "a<b",
            "a>b",
            "(a)",
            "a*",
            "a?",
            "[a]",
            "{a}",
            "~",
            "50%",
            "a!",
            "#a",
            "a\nb",
            "caf\u{e9}",
        ] {
            assert!(
                matches!(check_token(token), Err(TargetError::BadToken(_))),
                "{token:?} was accepted"
            );
        }
        for token in [
            "cox",
            "app-server",
            "--stdio",
            "/usr/bin/cox",
            "C:\\cox\\cox.exe",
            "a=b,c@d+e_f.g",
        ] {
            assert!(check_token(token).is_ok(), "{token:?} was refused");
        }
    }
}
