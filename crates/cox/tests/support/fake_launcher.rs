// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T68.9: the fake `ssh`, `docker`, `limactl` and `colima` every P68 test
//! launches through. Each is an `/bin/sh` script that records its argv and
//! environment, refuses an argv prefix other than the one the tool's own docs
//! give for `Tty::No`, drops that prefix and the remote `cox`, and runs the
//! built `cox` in its place. Included by `remote.rs` and `targets.rs` through
//! `#[path]` so there is one implementation of the stand-in, not one per test
//! file; it is not a `[[bin]]` like `fake_agent` because it needs no process
//! of its own.
//!
//! The prefix is written out here and not read back from
//! `cox_target::command`: a wrong argv in `command` must fail a test, and a
//! check derived from `command` would agree with it.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use cox_target::Kind;

/// The exit status a fake launcher uses for an argv it does not accept.
pub const BAD_ARGV: i32 = 97;

/// One position in the argv prefix.
enum Step {
    /// This exact word.
    Word(&'static str),
    /// The target's name (host, container, instance or profile).
    Name,
    /// `flag value` when the first word is `flag`, otherwise nothing.
    OptionalPair(&'static str),
}

/// What each launcher puts before the remote command when it has no
/// terminal: `ssh -T …`, `docker exec -i`, `limactl shell --tty=false`,
/// `colima ssh --profile`.
fn prefix(kind: Kind) -> Vec<Step> {
    use Step::{Name, OptionalPair, Word};
    match kind {
        Kind::Ssh => vec![
            Word("-T"),
            Word("-o"),
            Word("BatchMode=yes"),
            Word("-o"),
            Word("ForwardAgent=no"),
            Word("-o"),
            Word("ClearAllForwardings=yes"),
            Word("--"),
            Name,
        ],
        Kind::Docker => vec![OptionalPair("--context"), Word("exec"), Word("-i"), Name],
        Kind::Lima => vec![Word("shell"), Word("--tty=false"), Name],
        Kind::Colima => vec![Word("ssh"), Word("--profile"), Name, Word("--")],
    }
}

fn check(steps: &[Step]) -> String {
    steps
        .iter()
        .map(|step| match step {
            Step::Word(word) => format!("[ \"${{1-}}\" = '{word}' ] || fail\nshift\n"),
            Step::Name => "[ $# -gt 0 ] || fail\nshift\n".to_owned(),
            Step::OptionalPair(flag) => {
                format!("if [ \"${{1-}}\" = '{flag}' ]; then [ $# -ge 2 ] || fail; shift 2; fi\n")
            }
        })
        .collect()
}

/// Writes the fake launcher for `kind` into `dir` as an executable named
/// like the real program. It runs `cox` with `home` as the remote user's
/// home (`COX_HOME` is `home/.cox`) and the scripted provider on `scenario`,
/// set inside the script because the launcher's own environment was scrubbed,
/// as a real remote's settings would be.
pub fn write(dir: &Path, kind: Kind, cox: &Path, home: &Path, scenario: &Path) -> PathBuf {
    let program = kind.program();
    let path = dir.join(program);
    let script = format!(
        "#!/bin/sh\n\
         all=\"$*\"\n\
         printf '%s\\n' \"$@\" > \"{dir}/{program}-args\"\n\
         env > \"{dir}/{program}-env\"\n\
         fail() {{ echo \"fake {program}: unexpected argv: $all\" >&2; exit {BAD_ARGV}; }}\n\
         {check}\
         [ \"${{1-}}\" = cox ] || fail\n\
         shift\n\
         export HOME=\"{home}\" COX_HOME=\"{home}/.cox\" COX_KEYRING=off\n\
         export COX_PROVIDER=scripted COX_SCENARIO=\"{scenario}\"\n\
         exec \"{cox}\" \"$@\"\n",
        dir = dir.display(),
        check = check(&prefix(kind)),
        home = home.display(),
        scenario = scenario.display(),
        cox = cox.display(),
    );
    std::fs::write(&path, script).expect("write the fake launcher");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

/// The arguments the fake `kind` launcher was last started with.
pub fn recorded_args(dir: &Path, kind: Kind) -> Vec<String> {
    let text = std::fs::read_to_string(dir.join(format!("{}-args", kind.program())))
        .expect("the launcher recorded its argv");
    text.lines().map(str::to_owned).collect()
}

/// The environment the fake `kind` launcher was last started with, `env`'s
/// text.
pub fn recorded_env(dir: &Path, kind: Kind) -> String {
    std::fs::read_to_string(dir.join(format!("{}-env", kind.program())))
        .expect("the launcher recorded its environment")
}
