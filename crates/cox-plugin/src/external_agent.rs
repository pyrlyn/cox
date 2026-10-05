// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! A granted `[[external_agents]]` entry resolved to the process the host
//! spawns (EA§2, T35.2). Its own module for two reasons. The program
//! resolution — a PATH program as approved, or a regular file inside the
//! package reached through no symlink — is the one T33.19 gives a plugin's
//! `[[mcp]]` stdio server, so both call [`package_program`] here instead of
//! each keeping a copy. And the only constructor of [`ExternalAgentCommand`]
//! takes the caller's sandbox wrap, so an unwrapped external agent cannot be
//! built by accident: the host owns `sandbox::Policy`, this crate does not.
//! A user-config `[external_agents.<name>]` entry (T52.2) goes through the
//! same constructor with [`AgentSource::Config`], so both sources share it.

use std::ffi::OsStr;
use std::fmt::Display;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use cox_plugin_api::{AgentMode, ExternalAgentDecl};
use thiserror::Error;

/// Why a manifest `command` does not resolve to a program the grant covers.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProgramError {
    /// Absolute, or climbs out with `..`.
    #[error("command {0:?} must be a PATH program or a path inside the package")]
    Outside(String),
    /// A symlink on the way: the digest hashes regular files only.
    #[error("{0} is a symlink, which the package digest does not cover")]
    Symlink(String),
    /// Missing or unreadable.
    #[error("{path}: {message}")]
    Io {
        /// The path that failed.
        path: String,
        /// The OS error, as text.
        message: String,
    },
    /// Resolves to a directory or another non-file.
    #[error("{0} is not a file")]
    NotFile(String),
    /// A user-config `command` that is neither a bare PATH name nor an
    /// absolute path: a relative path would resolve against whatever
    /// directory the session opens in, which a repository controls.
    #[error("command {0:?} must be a PATH program or an absolute path")]
    NotAbsolute(String),
}

/// Where an entry was declared, handed to [`ExternalAgentCommand::resolve`].
#[derive(Debug, Clone, Copy)]
pub enum AgentSource<'a> {
    /// A granted plugin's `[[external_agents]]` (T35.2): `command` resolves
    /// inside the package at `dir` through [`package_program`].
    Plugin {
        /// The plugin id.
        id: &'a str,
        /// The package directory.
        dir: &'a Path,
    },
    /// The user's `[external_agents.<name>]` (T52.2): `command` is a PATH
    /// program or an absolute path; `writable` is the entry's extra state
    /// directories, already confined by the host, kept for display.
    Config {
        /// The entry's `writable` directories.
        writable: &'a [PathBuf],
    },
}

/// The owned form of [`AgentSource`] a resolved command keeps.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Origin {
    Plugin(String),
    Config(Vec<PathBuf>),
}

/// Why a granted `[[external_agents]]` entry is not offered this session.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExternalAgentError {
    /// `command` does not resolve to a program the grant covers.
    #[error(transparent)]
    Program(#[from] ProgramError),
    /// The sandbox wrap refused: plugin-shipped code never runs bare.
    #[error("cannot run under the sandbox: {0}")]
    Sandbox(String),
}

/// A bare name is a PATH program, approved as shown. Anything else must be
/// a regular file inside `dir`, reached through no symlink: the package
/// digest hashes regular files only, so a symlink would run bytes the grant
/// never covered.
pub fn package_program(dir: &Path, command: &str) -> Result<PathBuf, ProgramError> {
    let path = Path::new(command);
    let mut parts = path.components();
    if matches!(
        (parts.next(), parts.next()),
        (Some(Component::Normal(_)), None)
    ) {
        return Ok(path.to_path_buf());
    }
    if !path
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(ProgramError::Outside(command.to_string()));
    }
    let mut at = dir.to_path_buf();
    for part in path.components() {
        at.push(part);
        let meta = std::fs::symlink_metadata(&at).map_err(|e| ProgramError::Io {
            path: at.display().to_string(),
            message: e.to_string(),
        })?;
        if meta.file_type().is_symlink() {
            return Err(ProgramError::Symlink(at.display().to_string()));
        }
    }
    if !std::fs::metadata(&at).is_ok_and(|m| m.is_file()) {
        return Err(ProgramError::NotFile(at.display().to_string()));
    }
    Ok(at)
}

/// One granted entry, resolved and already wrapped by the host's sandbox:
/// what a driver spawns (EA§4 ACP, EA§5 stream-json). The fields are
/// private so the only way to one is [`ExternalAgentCommand::resolve`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAgentCommand {
    origin: Origin,
    name: String,
    mode: AgentMode,
    key_env: String,
    /// The agent's own program before the wrap: a bare PATH name or the
    /// in-package file `package_program` checked.
    cli: PathBuf,
    /// The agent's own args, before the wrap: what the launch line shows.
    cli_args: Vec<String>,
    program: String,
    args: Vec<String>,
}

/// A user-config `command`: a bare name is a PATH program, as for a
/// plugin; anything else must be an absolute path to a regular file.
fn config_program(command: &str) -> Result<PathBuf, ProgramError> {
    let path = Path::new(command);
    let mut parts = path.components();
    if matches!(
        (parts.next(), parts.next()),
        (Some(Component::Normal(_)), None)
    ) {
        return Ok(path.to_path_buf());
    }
    if !path.is_absolute() {
        return Err(ProgramError::NotAbsolute(command.to_string()));
    }
    let meta = std::fs::metadata(path).map_err(|e| ProgramError::Io {
        path: command.to_string(),
        message: e.to_string(),
    })?;
    if !meta.is_file() {
        return Err(ProgramError::NotFile(command.to_string()));
    }
    Ok(path.to_path_buf())
}

impl ExternalAgentCommand {
    /// Resolves `decl`'s program for its `source` — inside the package
    /// with [`package_program`] for a plugin, a PATH name or an absolute
    /// path for user config — and hands the program and its args to
    /// `wrap`, whose argv is what runs. A wrap error refuses the entry;
    /// there is no unwrapped fallback, whichever the source.
    pub fn resolve<E: Display>(
        source: AgentSource<'_>,
        decl: &ExternalAgentDecl,
        wrap: impl FnOnce(&Path, &[String]) -> Result<Vec<String>, E>,
    ) -> Result<Self, ExternalAgentError> {
        let (cli, origin) = match source {
            AgentSource::Plugin { id, dir } => (
                package_program(dir, &decl.command)?,
                Origin::Plugin(id.to_string()),
            ),
            AgentSource::Config { writable } => (
                config_program(&decl.command)?,
                Origin::Config(writable.to_vec()),
            ),
        };
        let mut argv = wrap(&cli, &decl.args)
            .map_err(|e| ExternalAgentError::Sandbox(e.to_string()))?
            .into_iter();
        let program = argv
            .next()
            .ok_or_else(|| ExternalAgentError::Sandbox(String::from("the wrap gave no program")))?;
        Ok(Self {
            origin,
            name: decl.name.clone(),
            mode: decl.mode,
            key_env: decl.key_env.clone(),
            cli,
            cli_args: decl.args.clone(),
            program,
            args: argv.collect(),
        })
    }

    /// The plugin that declared the entry; `None` for a user-config one.
    pub fn plugin(&self) -> Option<&str> {
        match &self.origin {
            Origin::Plugin(id) => Some(id),
            Origin::Config(_) => None,
        }
    }

    /// Where the entry came from, for a warning: `plugin <id>` or
    /// `user config`.
    pub fn origin(&self) -> String {
        match &self.origin {
            Origin::Plugin(id) => format!("plugin {id}"),
            Origin::Config(_) => String::from("user config"),
        }
    }

    /// The line the Agents list and a warning show (DT§3.3.1): the agent's
    /// own program and args before the wrap, its key variable, that it has
    /// network, and any extra writable directory a config entry lists.
    pub fn launch_line(&self) -> String {
        let mut line = std::iter::once(self.cli.display().to_string())
            .chain(self.cli_args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");
        line.push_str(&format!(" · key {} · network", self.key_env));
        if let Origin::Config(writable) = &self.origin
            && !writable.is_empty()
        {
            let dirs: Vec<String> = writable.iter().map(|d| d.display().to_string()).collect();
            line.push_str(&format!(" · writes {}", dirs.join(", ")));
        }
        line
    }

    /// The `agent(preset: <name>)` dispatch name (EA§3).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Which driver speaks to the process.
    pub fn mode(&self) -> AgentMode {
        self.mode
    }

    /// The env var the host resolves the key from (`resolve_key`, D12/A49).
    pub fn key_env(&self) -> &str {
        &self.key_env
    }

    /// The agent's program as the manifest names it, when it cannot run
    /// here: a bare name found in no `path` directory (the caller passes
    /// `PATH`). An in-package program was already checked by `resolve`.
    /// EA§7: such an entry is left out with one warning, never spawned to
    /// fail on every turn.
    pub fn missing_cli(&self, path: Option<&OsStr>) -> Option<&Path> {
        missing_on_path(&self.cli, path).then_some(self.cli.as_path())
    }

    /// The wrapped argv: the sandbox launcher first, the agent after it.
    pub fn argv(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.program.as_str()).chain(self.args.iter().map(String::as_str))
    }

    /// A fresh `Command` for the wrapped argv. Environment, stdio and the
    /// working directory are the driver's to set: the key goes in from
    /// `key_env` there, never through the manifest.
    pub fn command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args);
        cmd
    }
}

/// Whether `program`, as [`package_program`] resolved it, is a bare PATH
/// name found as an executable in no `path` directory. The one PATH lookup
/// for an external agent's CLI: the session leaves such an entry out
/// ([`ExternalAgentCommand::missing_cli`]) and `cox doctor` reports it
/// (T35.8), so the two can never disagree.
pub fn missing_on_path(program: &Path, path: Option<&OsStr>) -> bool {
    // `package_program` returns a bare name only for a PATH program; an
    // in-package one comes back joined onto the package directory.
    if program.components().count() > 1 {
        return false;
    }
    !path
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .any(|dir| is_executable(&dir.join(program)))
}

/// What `exec` would run: a regular file, with an execute bit on unix.
fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(command: &str) -> ExternalAgentDecl {
        ExternalAgentDecl {
            name: "cursor".into(),
            command: command.into(),
            args: vec!["acp".into()],
            mode: AgentMode::Acp,
            key_env: "CURSOR_API_KEY".into(),
        }
    }

    fn plugin(dir: &Path) -> AgentSource<'_> {
        AgentSource::Plugin { id: "cur", dir }
    }

    fn wrap(program: &Path, args: &[String]) -> Result<Vec<String>, String> {
        let mut argv = vec![String::from("sandbox-exec"), String::from("-p")];
        argv.push(program.display().to_string());
        argv.extend(args.iter().cloned());
        Ok(argv)
    }

    #[test]
    fn resolved_command_runs_the_wrap_not_the_program() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = ExternalAgentCommand::resolve(plugin(dir.path()), &decl("agent"), wrap)
            .expect("resolves");
        let cmd = agent.command();
        assert_eq!(cmd.get_program(), "sandbox-exec");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, ["-p", "agent", "acp"]);
        assert_eq!(
            agent.argv().collect::<Vec<_>>(),
            ["sandbox-exec", "-p", "agent", "acp"]
        );
        assert_eq!((agent.plugin(), agent.name()), (Some("cur"), "cursor"));
        assert_eq!(
            (agent.mode(), agent.key_env()),
            (AgentMode::Acp, "CURSOR_API_KEY")
        );
    }

    #[test]
    fn wrap_failure_refuses_the_agent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let refused = ExternalAgentCommand::resolve(plugin(dir.path()), &decl("agent"), |_, _| {
            Err::<Vec<String>, _>("no sandbox backend on this host")
        });
        assert_eq!(
            refused,
            Err(ExternalAgentError::Sandbox(
                "no sandbox backend on this host".into()
            ))
        );
        let empty = ExternalAgentCommand::resolve(plugin(dir.path()), &decl("agent"), |_, _| {
            Ok::<_, String>(Vec::new())
        });
        assert!(matches!(empty, Err(ExternalAgentError::Sandbox(_))));
    }

    #[test]
    fn in_package_program_resolves_inside_the_package() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("bin")).expect("mkdir");
        std::fs::write(dir.path().join("bin/agent"), b"#!/bin/sh\n").expect("agent");
        let agent = ExternalAgentCommand::resolve(plugin(dir.path()), &decl("bin/agent"), wrap)
            .expect("resolves");
        let inside = dir.path().join("bin/agent").display().to_string();
        assert_eq!(agent.argv().nth(2), Some(inside.as_str()));
        let missing = ExternalAgentCommand::resolve(plugin(dir.path()), &decl("bin/gone"), wrap);
        assert!(matches!(
            missing,
            Err(ExternalAgentError::Program(ProgramError::Io { .. }))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn missing_cli_is_a_path_name_found_in_no_directory() {
        use std::os::unix::fs::PermissionsExt as _;

        let pkg = tempfile::tempdir().expect("tempdir");
        let bin = tempfile::tempdir().expect("tempdir");
        let agent = ExternalAgentCommand::resolve(plugin(pkg.path()), &decl("agent"), wrap)
            .expect("resolves");
        let path = std::env::join_paths([bin.path()]).expect("path");
        assert_eq!(agent.missing_cli(Some(&path)), Some(Path::new("agent")));
        assert_eq!(agent.missing_cli(None), Some(Path::new("agent")));
        std::fs::write(bin.path().join("agent"), b"#!/bin/sh\n").expect("agent");
        assert!(agent.missing_cli(Some(&path)).is_some(), "not executable");
        std::fs::set_permissions(
            bin.path().join("agent"),
            std::fs::Permissions::from_mode(0o755),
        )
        .expect("chmod");
        assert_eq!(agent.missing_cli(Some(&path)), None);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_server_binary_is_refused() {
        let pkg = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(pkg.path().join("bin")).expect("mkdir");
        std::os::unix::fs::symlink("/bin/sh", pkg.path().join("bin/server")).expect("symlink");
        let err = package_program(pkg.path(), "bin/server").expect_err("not covered by the digest");
        assert!(matches!(err, ProgramError::Symlink(_)), "{err}");
        assert!(matches!(
            package_program(pkg.path(), "../outside"),
            Err(ProgramError::Outside(_))
        ));
        assert!(package_program(pkg.path(), "/bin/sh").is_err());
        assert_eq!(package_program(pkg.path(), "npx"), Ok(PathBuf::from("npx")));
    }

    /// T52.2 Check: a user-config entry goes through the same constructor
    /// and runs only as the wrap's argv. A wrap error refuses it, and a
    /// relative `command` (resolved against a directory a repository
    /// controls) is refused before any wrap is asked.
    #[test]
    fn config_external_agent_resolves_only_wrapped() {
        let writable = [PathBuf::from("/home/me/.claude")];
        let config = AgentSource::Config {
            writable: &writable,
        };
        let claude = ExternalAgentDecl {
            name: "claude".into(),
            command: "claude-agent-acp".into(),
            args: vec!["--hide-claude-auth".into()],
            mode: AgentMode::Acp,
            key_env: "ANTHROPIC_API_KEY".into(),
        };
        let agent = ExternalAgentCommand::resolve(config, &claude, wrap).expect("resolves");
        assert_eq!(agent.command().get_program(), "sandbox-exec");
        assert_eq!(
            agent.argv().collect::<Vec<_>>(),
            [
                "sandbox-exec",
                "-p",
                "claude-agent-acp",
                "--hide-claude-auth"
            ]
        );
        assert_eq!(agent.plugin(), None);
        assert_eq!(agent.origin(), "user config");
        assert_eq!(
            agent.launch_line(),
            "claude-agent-acp --hide-claude-auth · key ANTHROPIC_API_KEY · network \
             · writes /home/me/.claude"
        );

        let refused = ExternalAgentCommand::resolve(config, &claude, |_, _| {
            Err::<Vec<String>, _>("no sandbox backend on this host")
        });
        assert!(matches!(refused, Err(ExternalAgentError::Sandbox(_))));

        let mut relative = claude.clone();
        relative.command = "bin/claude-agent-acp".into();
        let mut asked = false;
        let refused = ExternalAgentCommand::resolve(config, &relative, |p, a| {
            asked = true;
            wrap(p, a)
        });
        assert!(matches!(
            refused,
            Err(ExternalAgentError::Program(ProgramError::NotAbsolute(_)))
        ));
        assert!(!asked, "a refused program never reaches the wrap");

        let mut gone = claude;
        gone.command = "/nonexistent/cox-no-such-agent".into();
        assert!(matches!(
            ExternalAgentCommand::resolve(config, &gone, wrap),
            Err(ExternalAgentError::Program(ProgramError::Io { .. }))
        ));
    }
}
