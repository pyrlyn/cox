// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `--on <target>` (T68.6, plan.md A158): run the TUI or `cox run` on a
//! target through its launcher. Separate from `run.rs` and `session.rs`
//! because nothing here opens a session: the model, tools, sandbox and keys
//! all stay on the target, and this side only builds one remote argv and
//! passes the terminal (or the prompt and stdin) through. That argv is the
//! trust boundary: only an allow-list of flags crosses, each value checked by
//! `cox_target::command`, and the prompt travels on stdin, never on the
//! command line.

use std::io::Write as _;
use std::process::Stdio;

use cox_protocol::Config;
use cox_target::Tty;

use crate::cli::{Cli, Command, RunArgs};

/// What to start on the target.
#[derive(Debug, PartialEq, Eq)]
struct Plan {
    /// The remote `cox` and its arguments.
    remote: Vec<String>,
    /// The TUI needs a terminal; a headless run is a pipe.
    tty: Tty,
    /// For `cox run`, sent as the first stdin line.
    prompt: Option<String>,
}

/// Flags set here that the target would never see, by name.
fn dropped(cli: &Cli, run: Option<&RunArgs>) -> Vec<&'static str> {
    let mut flags = vec![
        ("--provider", cli.provider.is_some()),
        ("--tier", !cli.tier.is_empty()),
        ("--approve", cli.approve.is_some()),
        ("--budget", cli.budget.is_some()),
        ("--profile", cli.profile.is_some()),
        ("--cwd", cli.cwd.is_some()),
        ("--add-dir", !cli.add_dir.is_empty()),
        ("--worktree", cli.worktree.is_some()),
        ("--home", cli.home.is_some()),
        ("--no-hooks", cli.no_hooks),
        ("--no-mcp", cli.no_mcp),
        ("--no-plugins", cli.no_plugins),
        ("--plain", cli.plain),
    ];
    match run {
        Some(run) => flags.extend([
            ("--prompt-stdin", run.prompt_stdin),
            ("--allowed-tools", run.allowed_tools.is_some()),
            ("--answer", run.answer.is_some()),
            ("--deep", run.deep),
            ("--loop", run.r#loop.is_some()),
            ("--max-iterations", run.max_iterations.is_some()),
            ("--image", !run.images.is_empty()),
            ("--resume", cli.resume.is_some()),
            ("--continue", cli.r#continue),
        ]),
        None => flags.push(("<PROMPT>", cli.prompt.is_some())),
    }
    flags
        .into_iter()
        .filter_map(|(name, set)| set.then_some(name))
        .collect()
}

fn flag(remote: &mut Vec<String>, name: &str, value: Option<String>) {
    if let Some(value) = value {
        remote.extend([name.to_owned(), value]);
    }
}

fn plan(cli: &Cli, cwd: Option<&str>) -> anyhow::Result<Plan> {
    let run = match &cli.command {
        None => None,
        Some(Command::Run(args)) => Some(args),
        Some(_) => anyhow::bail!("--on works with the TUI and `cox run` only"),
    };
    let dropped = dropped(cli, run);
    if !dropped.is_empty() {
        anyhow::bail!(
            "--on does not forward {}; set them in the target's own config",
            dropped.join(", ")
        );
    }
    let mut remote = vec!["cox".to_owned()];
    flag(&mut remote, "--cwd", cwd.map(str::to_owned));
    flag(&mut remote, "--model", cli.model.clone());
    flag(&mut remote, "--mode", cli.mode.clone());
    flag(
        &mut remote,
        "--permission-mode",
        cli.permission_mode.clone(),
    );
    flag(&mut remote, "--sandbox", cli.sandbox.clone());
    let Some(run) = run else {
        flag(&mut remote, "--resume", cli.resume.clone());
        if cli.r#continue {
            remote.push("--continue".to_owned());
        }
        return Ok(Plan {
            remote,
            tty: Tty::Yes,
            prompt: None,
        });
    };
    let prompt = run
        .prompt
        .clone()
        .ok_or_else(|| anyhow::anyhow!("--on needs a prompt: `cox run --on <target> -p <text>`"))?;
    remote.extend(["run".to_owned(), "--prompt-stdin".to_owned()]);
    flag(
        &mut remote,
        "--max-turns",
        run.max_turns.map(|n| n.to_string()),
    );
    flag(&mut remote, "--output-format", run.output_format.clone());
    flag(&mut remote, "--resume", run.resume.clone());
    if run.r#continue {
        remote.push("--continue".to_owned());
    }
    Ok(Plan {
        remote,
        tty: Tty::No,
        prompt: Some(prompt),
    })
}

/// Runs the session on `on` and returns the remote exit code.
pub fn run(cli: &Cli, config: &Config, on: &str) -> anyhow::Result<i32> {
    let resolved = cox_session::targets::resolve(config, on)?;
    let plan = plan(cli, resolved.cwd.as_deref())?;
    let argv: Vec<&str> = plan.remote.iter().map(String::as_str).collect();
    let mut command = cox_target::command(&resolved.target, &resolved.launcher, &argv, plan.tty)?;
    let Some(prompt) = plan.prompt else {
        // The terminal is the launcher's: stdio is inherited as it is.
        return exit_code(command.status()?);
    };
    let mut child = command
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("cannot start {}: {e}", resolved.launcher.display()))?;
    let mut remote_stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("the launcher has no stdin"))?;
    // why: a launcher that exits before reading the prompt (a refused argv,
    // an unreachable host) closes the pipe; its exit code says why, our
    // EPIPE does not.
    match writeln!(remote_stdin, "{}", serde_json::to_string(&prompt)?) {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => return exit_code(child.wait()?),
        written => written?,
    }
    // why: our stdin is the remote approvals channel; the thread is left to
    // die with the process, as a terminal stdin never reaches EOF.
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin(), &mut remote_stdin);
    });
    exit_code(child.wait()?)
}

/// A launcher killed by a signal has no code, which is a failure to a script.
fn exit_code(status: std::process::ExitStatus) -> anyhow::Result<i32> {
    Ok(status.code().unwrap_or(crate::run::EXIT_ERROR))
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::*;

    fn plan_of(args: &[&str]) -> anyhow::Result<Plan> {
        plan(&Cli::parse_from(args), Some("/work"))
    }

    #[test]
    fn a_windows_cwd_reaches_the_remote_command_line_whole() {
        let cli = Cli::parse_from(["cox", "--on", "ssh:winbox"]);
        let plan = plan(&cli, Some("C:\\srv\\app")).unwrap();
        assert_eq!(plan.remote, ["cox", "--cwd", "C:\\srv\\app"]);
    }

    #[test]
    fn tui_on_a_target_asks_for_a_tty() {
        let plan = plan_of(&["cox", "--on", "lima:dev", "--model", "m1"]).unwrap();
        assert_eq!(plan.tty, Tty::Yes);
        assert_eq!(plan.remote, ["cox", "--cwd", "/work", "--model", "m1"]);
        assert_eq!(plan.prompt, None);
    }

    #[test]
    fn run_on_a_target_keeps_the_prompt_off_the_command_line() {
        let plan = plan_of(&[
            "cox",
            "run",
            "--on",
            "docker:ci",
            "-p",
            "fix; rm -rf /",
            "--max-turns",
            "3",
            "--output-format",
            "stream-json",
            "--sandbox",
            "workspace-write",
        ])
        .unwrap();
        assert_eq!(plan.tty, Tty::No);
        assert_eq!(plan.prompt.as_deref(), Some("fix; rm -rf /"));
        assert_eq!(
            plan.remote,
            [
                "cox",
                "--cwd",
                "/work",
                "--sandbox",
                "workspace-write",
                "run",
                "--prompt-stdin",
                "--max-turns",
                "3",
                "--output-format",
                "stream-json"
            ]
        );
    }

    #[test]
    fn resume_and_continue_are_forwarded() {
        let tui = plan_of(&["cox", "--on", "a:b", "--resume", "s1"]).unwrap();
        assert_eq!(tui.remote[tui.remote.len() - 2..], ["--resume", "s1"]);
        let run = plan_of(&["cox", "run", "--on", "a:b", "-p", "x", "--continue"]).unwrap();
        assert_eq!(run.remote.last().map(String::as_str), Some("--continue"));
    }

    #[test]
    fn flags_outside_the_allow_list_are_refused_by_name() {
        let error = plan_of(&[
            "cox", "run", "--on", "a:b", "-p", "x", "--cwd", "/tmp", "--deep",
        ])
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("--cwd") && error.contains("--deep"),
            "{error}"
        );
        assert!(plan_of(&["cox", "--on", "a:b", "first prompt"]).is_err());
    }

    #[test]
    fn other_subcommands_and_a_missing_prompt_are_refused() {
        assert!(plan_of(&["cox", "--on", "a:b", "sessions"]).is_err());
        assert!(plan_of(&["cox", "--on", "a:b", "run"]).is_err());
    }
}
