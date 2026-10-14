// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! From what a person typed to something launchable (T68.2): a configured
//! name or a literal `kind:name` becomes a [`Target`], the directory to start
//! in and the absolute launcher program. Separate from `cox-config` and
//! `cox-session` so the checks (a launcher path is absolute and exists, a
//! `cwd` is a safe token, a docker `context` stays on docker) have one owner
//! and take plain borrowed values: the caller reads its own config, and a
//! test injects what exists on disk.

use std::path::{Path, PathBuf};

use crate::{Kind, Target, TargetError, check_name, check_token, launcher};

/// One configured target, borrowed from the caller's config.
#[derive(Debug, Clone, Copy)]
pub struct Entry<'a> {
    /// Where it runs, as `kind:name`.
    pub target: &'a str,
    /// Directory on the target the session starts in.
    pub cwd: Option<&'a str>,
    /// `docker --context`; docker targets only.
    pub context: Option<&'a str>,
    /// Absolute launcher path for this target alone.
    pub program: Option<&'a Path>,
}

/// A target ready to launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// Where the session runs.
    pub target: Target,
    /// The launcher program, absolute and present.
    pub launcher: PathBuf,
    /// Directory on the target to start in, already checked for the remote
    /// command line.
    pub cwd: Option<String>,
}

/// Resolves `text`: a name `entry` knows wins over reading it as
/// `kind:name`, so a configured `web` cannot be shadowed by a literal. The
/// launcher is the entry's `program`, else `launcher_override(kind)`, else
/// the fixed lookup; the first two must be absolute and `exists`, because a
/// relative path would be found through the working directory.
pub fn resolve<'e>(
    text: &str,
    entry: impl Fn(&str) -> Option<Entry<'e>>,
    launcher_override: impl Fn(Kind) -> Option<PathBuf>,
    exists: impl Fn(&Path) -> bool,
) -> Result<Resolved, TargetError> {
    let entry = entry(text);
    let mut target: Target = entry.map_or(text, |e| e.target).parse()?;
    if let Some(context) = entry.and_then(|e| e.context) {
        let Target::Docker { context: slot, .. } = &mut target else {
            return Err(TargetError::ContextNeedsDocker(target.to_string()));
        };
        // why: the context is local argv for `docker --context`, never remote.
        check_name(context)?;
        *slot = Some(context.to_owned());
    }
    let cwd = entry.and_then(|e| e.cwd);
    if let Some(cwd) = cwd {
        check_token(cwd)?;
    }
    let launcher = match entry
        .and_then(|e| e.program.map(Path::to_path_buf))
        .or_else(|| launcher_override(target.kind()))
    {
        Some(path) => configured(path, &exists)?,
        None => launcher::find(target.kind(), &exists)
            .ok_or(TargetError::NoLauncher(target.kind().program()))?,
    };
    Ok(Resolved {
        target,
        launcher,
        cwd: cwd.map(str::to_owned),
    })
}

fn configured(path: PathBuf, exists: &impl Fn(&Path) -> bool) -> Result<PathBuf, TargetError> {
    if !path.is_absolute() {
        return Err(TargetError::RelativeLauncher(path));
    }
    if !exists(&path) {
        return Err(TargetError::MissingLauncher(path));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEB: Entry<'static> = Entry {
        target: "docker:web",
        cwd: None,
        context: None,
        program: None,
    };

    fn none(_: &str) -> Option<Entry<'static>> {
        None
    }

    fn no_override(_: Kind) -> Option<PathBuf> {
        None
    }

    #[test]
    fn literal_target_uses_the_first_existing_candidate() {
        let want = launcher::candidates(Kind::Ssh).remove(0);
        let got = resolve("ssh:devbox", none, no_override, |p| p == want).expect("resolves");
        assert_eq!(got.target.to_string(), "ssh:devbox");
        assert_eq!(got.launcher, want);
        assert_eq!(got.cwd, None);
    }

    #[test]
    fn configured_name_beats_a_literal() {
        let entry = |name: &str| (name == "ssh:devbox").then_some(WEB);
        let got = resolve("ssh:devbox", entry, no_override, |_| true).expect("resolves");
        assert_eq!(got.target.to_string(), "docker:web");
    }

    #[test]
    fn docker_context_is_applied_and_only_to_docker() {
        let with = |target| Entry {
            context: Some("staging"),
            target,
            ..WEB
        };
        let got =
            resolve("web", |_| Some(with("docker:web")), no_override, |_| true).expect("resolves");
        assert_eq!(
            got.target,
            Target::Docker {
                container: "web".into(),
                context: Some("staging".into())
            }
        );
        let err = resolve("box", |_| Some(with("ssh:box")), no_override, |_| true)
            .expect_err("ssh has no context");
        assert!(matches!(err, TargetError::ContextNeedsDocker(_)), "{err}");
        let bad = Entry {
            context: Some("-H x"),
            ..WEB
        };
        assert!(resolve("web", |_| Some(bad), no_override, |_| true).is_err());
    }

    #[test]
    fn cwd_must_be_safe_on_a_remote_command_line() {
        let entry = |cwd| move |_: &str| Some(Entry { cwd, ..WEB });
        let ok = resolve("web", entry(Some("/srv/app")), no_override, |_| true).expect("safe cwd");
        assert_eq!(ok.cwd.as_deref(), Some("/srv/app"));
        for cwd in ["/srv/my app", "/srv;rm", "$HOME", ""] {
            let err = resolve("web", entry(Some(cwd)), no_override, |_| true)
                .expect_err("unsafe cwd was accepted");
            assert!(matches!(err, TargetError::BadToken(_)), "{cwd:?}: {err}");
        }
    }

    #[test]
    fn configured_launcher_must_be_absolute_and_present() {
        let program = |p: &'static str| {
            move |_: &str| {
                Some(Entry {
                    program: Some(Path::new(p)),
                    ..WEB
                })
            }
        };
        let err = resolve("web", program("docker"), no_override, |_| true).expect_err("relative");
        assert!(matches!(err, TargetError::RelativeLauncher(_)), "{err}");
        let relative = |_: Kind| Some(PathBuf::from("bin/docker"));
        let err = resolve("docker:web", none, relative, |_| true).expect_err("relative override");
        assert!(matches!(err, TargetError::RelativeLauncher(_)), "{err}");
        let err =
            resolve("web", program("/opt/docker"), no_override, |_| false).expect_err("missing");
        assert!(matches!(err, TargetError::MissingLauncher(_)), "{err}");
        let got = resolve("web", program("/opt/docker"), no_override, |p| {
            p == Path::new("/opt/docker")
        })
        .expect("present");
        assert_eq!(got.launcher, Path::new("/opt/docker"));
    }

    #[test]
    fn launcher_override_beats_the_lookup_but_not_the_entry_program() {
        let by_kind = |k: Kind| (k == Kind::Docker).then(|| PathBuf::from("/opt/a/docker"));
        let got = resolve("docker:web", none, by_kind, |_| true).expect("resolves");
        assert_eq!(got.launcher, Path::new("/opt/a/docker"));
        let own = Entry {
            program: Some(Path::new("/opt/b/docker")),
            ..WEB
        };
        let got = resolve("web", |_| Some(own), by_kind, |_| true).expect("resolves");
        assert_eq!(got.launcher, Path::new("/opt/b/docker"));
    }

    #[test]
    fn no_launcher_anywhere_is_one_readable_error() {
        let err = resolve("lima:default", none, no_override, |_| false).expect_err("none");
        assert!(matches!(err, TargetError::NoLauncher("limactl")), "{err}");
    }
}
