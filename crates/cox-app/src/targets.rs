// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The targets the desktop app can connect to (T68.4): the configured ones
//! plus the containers and VMs running right now, and the connection to one.
//! Separate from `remote.rs`, which owns the wire, because this side only
//! asks the launchers what is running and resolves a name through
//! `cox_session::targets`. A listing runs the launcher with a short timeout
//! and fails open: no launcher, a hung daemon or unreadable output lists
//! nothing for that kind and is never an error.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cox_target::{Kind, Target};

use crate::app::App;
use crate::remote::{RemoteError, RemoteWorkspace};

/// How long one launcher may take to list: a stopped Docker Desktop hangs
/// `docker ps` for much longer, and the picker must not wait on it.
const LISTING_TIMEOUT: Duration = Duration::from_secs(3);

/// One target a person can pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetChoice {
    /// What the picker shows: the configured name, or the target text.
    pub name: String,
    /// What `connect_target` takes.
    pub target: String,
    /// Named in config, as opposed to found running.
    pub configured: bool,
}

impl App {
    /// The configured targets, then the running containers and VMs they do
    /// not already name.
    pub async fn targets(&self) -> Vec<TargetChoice> {
        let Ok(config) = self.config(&self.home) else {
            return Vec::new();
        };
        let launcher = |kind| cox_session::targets::launcher(&config, kind);
        let (docker, lima, colima) = tokio::join!(
            running(Kind::Docker, launcher(Kind::Docker)),
            running(Kind::Lima, launcher(Kind::Lima)),
            running(Kind::Colima, launcher(Kind::Colima)),
        );
        let mut choices: Vec<TargetChoice> = cox_session::targets::configured(&config)
            .into_iter()
            .map(|(name, target)| TargetChoice {
                name,
                target,
                configured: true,
            })
            .collect();
        for target in docker.into_iter().chain(lima).chain(colima) {
            let text = target.to_string();
            if choices.iter().all(|c| c.target != text) {
                choices.push(TargetChoice {
                    name: text.clone(),
                    target: text,
                    configured: false,
                });
            }
        }
        choices
    }

    /// A workspace on `text`: a configured name or a `kind:name`. Its inbox
    /// items and links reach this app's host.
    pub async fn connect_target(&self, text: &str) -> Result<Arc<RemoteWorkspace>, RemoteError> {
        let config = self
            .config(&self.home)
            .map_err(|e| RemoteError::Config(e.to_string()))?;
        let resolved = cox_session::targets::resolve(&config, text)?;
        RemoteWorkspace::connect(resolved, Arc::clone(&self.host)).await
    }
}

async fn running(kind: Kind, launcher: Option<PathBuf>) -> Vec<Target> {
    let Some(command) = launcher.and_then(|l| cox_target::listing(kind, &l)) else {
        return Vec::new();
    };
    let mut command = tokio::process::Command::from(command);
    // why: a timed-out listing is dropped mid-run, and must not outlive it.
    command
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(LISTING_TIMEOUT, command.output()).await {
        Ok(Ok(out)) if out.status.success() => {
            cox_target::parse_listing(kind, &String::from_utf8_lossy(&out.stdout))
        }
        _ => Vec::new(),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;

    use super::*;
    use crate::InboxItem;
    use crate::app::Host;

    struct Quiet;

    impl Host for Quiet {
        fn notify(&self, _: InboxItem, _: u32) {}
        fn badge(&self, _: u32) {}
        fn open_url(&self, _: &str) {}
        fn secret(&self, _: &str) -> Option<String> {
            None
        }
    }

    fn script(path: &Path, body: &str) {
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write the script");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    /// An app whose config points every launcher at `dir`: the fake `docker`
    /// when `docker` is a script body, a missing file otherwise.
    fn app(dir: &Path, docker: Option<&str>) -> Arc<App> {
        let path = |name: &str| dir.join(name);
        if let Some(body) = docker {
            script(&path("docker"), body);
        }
        let config = format!(
            "[launchers]\nssh = {:?}\ndocker = {:?}\nlimactl = {:?}\ncolima = {:?}\n\n\
             [targets.web]\ntarget = \"docker:web\"\n",
            path("ssh"),
            path("docker"),
            path("limactl"),
            path("colima"),
        );
        std::fs::write(dir.join("config.toml"), config).expect("write the config");
        App::new(Some(dir.to_owned()), Arc::new(Quiet)).expect("an app")
    }

    #[tokio::test]
    async fn listing_skips_a_missing_launcher() {
        let dir = tempfile::tempdir().expect("tempdir");
        let choices = app(dir.path(), None).targets().await;
        assert_eq!(
            choices,
            [TargetChoice {
                name: "web".into(),
                target: "docker:web".into(),
                configured: true
            }]
        );
    }

    #[tokio::test]
    async fn running_containers_join_the_configured_targets() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ps = r#"printf '%s\n' '{"Names":"web"}' '{"Names":"db"}'"#;
        let names = |choices: Vec<TargetChoice>| {
            choices
                .into_iter()
                .map(|c| (c.target, c.configured))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(app(dir.path(), Some(ps)).targets().await),
            [
                ("docker:web".to_owned(), true),
                ("docker:db".to_owned(), false)
            ]
        );
    }

    #[tokio::test]
    async fn a_failing_listing_lists_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let failing = "echo '{\"Names\":\"db\"}'\nexit 1";
        let choices = app(dir.path(), Some(failing)).targets().await;
        assert_eq!(choices.len(), 1, "{choices:?}");
    }
}
