// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `[targets.<name>]`, `[launchers]` and `desktop.remote_hosts` read as run
//! targets (T68.2): the one place a `Config` becomes the plain values
//! `cox_target::resolve` checks, so `--on`, the desktop app and the `agent`
//! presets name targets the same way. Separate from `cox-target`, which
//! depends on no workspace crate and so never sees a `Config`.

use std::path::{Path, PathBuf};

use cox_protocol::Config;
use cox_target::{Entry, Kind, Resolved, TargetError};

/// `text` — a configured name, a `desktop.remote_hosts` alias or a literal
/// `kind:name` — as a target with its launcher and starting directory.
pub fn resolve(config: &Config, text: &str) -> Result<Resolved, TargetError> {
    resolve_with(config, text, Path::is_file)
}

/// Every name a person can pick, as `(display name, target text)`: each
/// `[targets.*]` entry, then each `desktop.remote_hosts` alias as
/// `ssh:<alias>` unless a target already took that name.
pub fn configured(config: &Config) -> Vec<(String, String)> {
    let named = config
        .targets
        .iter()
        .map(|(name, t)| (name.clone(), t.target.clone()));
    let hosts = config
        .desktop
        .remote_hosts
        .iter()
        .filter(|alias| !config.targets.contains_key(*alias))
        .map(|alias| (alias.clone(), format!("ssh:{alias}")));
    named.chain(hosts).collect()
}

/// The launcher a listing runs: the configured path if it is an absolute
/// file, else the fixed lookup. `None` for any miss, because a listing
/// fails open (T68.4); a connection reports the same miss as an error.
pub fn launcher(config: &Config, kind: Kind) -> Option<PathBuf> {
    match launcher_override(config, kind) {
        Some(path) => (path.is_absolute() && path.is_file()).then_some(path),
        None => cox_target::launcher(kind).ok(),
    }
}

fn launcher_override(config: &Config, kind: Kind) -> Option<PathBuf> {
    let launchers = &config.launchers;
    match kind {
        Kind::Ssh => &launchers.ssh,
        Kind::Docker => &launchers.docker,
        Kind::Lima => &launchers.limactl,
        Kind::Colima => &launchers.colima,
    }
    .clone()
}

fn resolve_with(
    config: &Config,
    text: &str,
    exists: impl Fn(&Path) -> bool,
) -> Result<Resolved, TargetError> {
    // why: `Entry` borrows its target text, and an alias has none until it
    // is written as `ssh:<alias>`.
    let hosts: Vec<(&str, String)> = config
        .desktop
        .remote_hosts
        .iter()
        .map(|alias| (alias.as_str(), format!("ssh:{alias}")))
        .collect();
    let entry = |name: &str| {
        if let Some(t) = config.targets.get(name) {
            return Some(Entry {
                target: &t.target,
                cwd: t.cwd.as_deref(),
                context: t.context.as_deref(),
                program: t.program.as_deref(),
            });
        }
        hosts
            .iter()
            .find(|(alias, _)| *alias == name)
            .map(|(_, target)| Entry {
                target,
                cwd: None,
                context: None,
                program: None,
            })
    };
    let by_kind = |kind: Kind| launcher_override(config, kind);
    cox_target::resolve(text, entry, by_kind, exists)
}

#[cfg(test)]
mod tests {
    use cox_protocol::Config;
    use cox_protocol::config::TargetConfig;

    use super::*;

    fn target(text: &str) -> TargetConfig {
        TargetConfig {
            target: text.to_owned(),
            ..TargetConfig::default()
        }
    }

    #[test]
    fn remote_hosts_read_as_ssh_targets() {
        let mut config = Config::default();
        config.desktop.remote_hosts = vec!["devbox".into(), "lab".into()];
        config.targets.insert("lab".into(), target("docker:lab"));
        let got = resolve_with(&config, "devbox", |_| true).expect("an alias resolves");
        assert_eq!(got.target.to_string(), "ssh:devbox");
        assert_eq!(
            configured(&config),
            [
                ("lab".to_owned(), "docker:lab".to_owned()),
                ("devbox".to_owned(), "ssh:devbox".to_owned())
            ]
        );
    }

    #[test]
    fn configured_name_wins_over_literal() {
        let mut config = Config::default();
        config
            .targets
            .insert("ssh:devbox".into(), target("lima:vm"));
        let got = resolve_with(&config, "ssh:devbox", |_| true).expect("resolves");
        assert_eq!(got.target.to_string(), "lima:vm");
        let literal = resolve_with(&Config::default(), "ssh:devbox", |_| true).expect("literal");
        assert_eq!(literal.target.to_string(), "ssh:devbox");
    }

    #[test]
    fn relative_launcher_is_refused() {
        let mut config = Config::default();
        config.launchers.docker = Some(PathBuf::from("docker"));
        let err = resolve_with(&config, "docker:web", |_| true).expect_err("relative");
        assert!(matches!(err, TargetError::RelativeLauncher(_)), "{err}");
        config.launchers.docker = Some(PathBuf::from("/opt/docker"));
        let got = resolve_with(&config, "docker:web", |_| true).expect("absolute");
        assert_eq!(got.launcher, Path::new("/opt/docker"));
    }

    #[test]
    fn cwd_with_spaces_is_refused() {
        let mut config = Config::default();
        config.targets.insert(
            "web".into(),
            TargetConfig {
                cwd: Some("/srv/my app".into()),
                ..target("docker:web")
            },
        );
        let err = resolve_with(&config, "web", |_| true).expect_err("space in cwd");
        assert!(matches!(err, TargetError::BadToken(_)), "{err}");
    }
}
