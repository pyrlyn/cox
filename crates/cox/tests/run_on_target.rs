// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T68.6: `cox run --on <target>` against the real binary and a fake
//! `docker`/`ssh` (T68.9). The local `cox` finds the launcher through the
//! user config, runs the remote `cox run --prompt-stdin` through it and must
//! hand over the prompt on stdin only, copy stdout through and exit with the
//! remote's code.

#![cfg(unix)]

use std::path::Path;

use assert_cmd::Command;
use cox_target::Kind;

// why: `remote.rs` and `targets.rs` share this file; not every helper is used here.
#[allow(dead_code)]
#[path = "support/fake_launcher.rs"]
mod fake_launcher;

const PROMPT: &str = "ZEBRA-PROMPT-7 do the thing";

/// A scratch machine: a local home whose `config.toml` points the `ci` target
/// at `launcher` through `launcher_key`, a remote home and a work directory.
struct Rig {
    dir: tempfile::TempDir,
}

impl Rig {
    fn new(launcher_key: &str, kind: Kind, target: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        for sub in ["local", "remote/.cox", "work/.git", "bin"] {
            std::fs::create_dir_all(root.join(sub)).expect("mkdir");
        }
        let scenario = root.join("scenario.toml");
        std::fs::write(
            &scenario,
            "[[turn]]\ntext = \"got it\"\nwhen_contains = \"ZEBRA-PROMPT-7\"\n",
        )
        .expect("scenario");
        let cox = Path::new(env!("CARGO_BIN_EXE_cox"));
        let launcher = fake_launcher::write(
            &root.join("bin"),
            kind,
            cox,
            &root.join("remote"),
            &scenario,
        );
        std::fs::write(
            root.join("local/config.toml"),
            format!(
                "[launchers]\n{launcher_key} = \"{}\"\n[targets.ci]\ntarget = \"{target}\"\ncwd = \"{}\"\n",
                launcher.display(),
                root.join("work").display()
            ),
        )
        .expect("config");
        Self { dir }
    }

    fn cox(&self) -> Command {
        self.cox_with(PROMPT)
    }

    fn cox_with(&self, prompt: &str) -> Command {
        let root = self.dir.path();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cox"));
        cmd.current_dir(root.join("work"))
            .env("COX_HOME", root.join("local"))
            .env("HOME", root.join("local"))
            .env("COX_KEYRING", "off")
            .env_remove("COX_PROVIDER")
            .args(["run", "--on", "ci", "-p", prompt])
            .write_stdin("");
        cmd
    }
}

#[test]
fn run_on_target_sends_prompt_over_stdin() {
    let rig = Rig::new("docker", Kind::Docker, "docker:ci");
    rig.cox().assert().success().stdout("got it\n");
    let args = fake_launcher::recorded_args(&rig.dir.path().join("bin"), Kind::Docker);
    assert!(
        args.iter().all(|a| !a.contains("ZEBRA")),
        "the prompt reached the command line: {args:?}"
    );
    assert!(args.contains(&"--prompt-stdin".to_owned()), "{args:?}");
    let work = rig.dir.path().join("work");
    let cwd = args.iter().position(|a| a == "--cwd").expect("--cwd");
    assert_eq!(args[cwd + 1], work.to_str().unwrap());
}

#[test]
fn run_on_target_returns_remote_exit_code() {
    // An `ssh:` target through the fake docker: the script refuses the argv
    // and exits 97, which must come back unchanged, with its message.
    let rig = Rig::new("ssh", Kind::Docker, "ssh:ci");
    rig.cox()
        .assert()
        .code(fake_launcher::BAD_ARGV)
        .stderr(predicates::str::contains("unexpected argv"));
}

#[test]
fn a_launcher_that_exits_before_reading_the_prompt_keeps_its_exit_code() {
    // A prompt larger than any pipe buffer cannot be written before the
    // launcher, which refuses the argv unread, exits: the write always meets
    // a closed pipe, which used to turn exit 97 into our own error.
    let rig = Rig::new("ssh", Kind::Docker, "ssh:ci");
    let prompt = format!("{PROMPT} {}", "x".repeat(100 * 1024));
    rig.cox_with(&prompt)
        .assert()
        .code(fake_launcher::BAD_ARGV)
        .stderr(predicates::str::contains("unexpected argv"));
}

#[test]
fn run_on_target_refuses_a_flag_it_does_not_forward() {
    let rig = Rig::new("docker", Kind::Docker, "docker:ci");
    rig.cox()
        .args(["--provider", "anthropic"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("--provider"));
}
