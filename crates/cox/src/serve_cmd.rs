// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox serve`, its hidden `--worker` and `cox attach --stdio` (T66.23,
//! `docs/design/serve.md` S8): starts `cox-serve`'s supervisor in the
//! foreground, runs one worker's session loop, and bridges stdio to a running
//! supervisor's socket. Its own module because it is the one place `cox-serve`
//! and `cox-app` meet: `cox-serve` may not link the app core, so the
//! supervisor's workspace calls reach `cox_app` through the `Workspace`
//! impl below. Unix only; Windows has no socket transport yet (D4).

use std::path::Path;

#[cfg(unix)]
use std::sync::Arc;

#[cfg(unix)]
use cox_app::app::App;
#[cfg(unix)]
use cox_serve::{Options, Workspace, attach, router, socket, terminated};

/// The supervisor's workspace calls, answered by `cox-app`'s wire code.
#[cfg(unix)]
struct AppWorkspace(Arc<App>);

#[cfg(unix)]
impl Workspace for AppWorkspace {
    fn call(&self, call: &serde_json::Value) -> Result<serde_json::Value, String> {
        cox_app::server::workspace_reply(&self.0, call)
    }
}

/// `cox serve`: the supervisor until SIGINT or SIGTERM; with `worker`, the
/// one-session loop the supervisor starts for each resident session.
pub fn serve(home: &Path, worker: bool) -> anyhow::Result<()> {
    #[cfg(not(unix))]
    {
        let _ = (home, worker);
        anyhow::bail!("cox serve is not supported yet on this platform");
    }
    #[cfg(unix)]
    {
        let runtime = tokio::runtime::Runtime::new()?;
        if worker {
            let mode = cox_app::server::ServeMode::Worker;
            runtime.block_on(cox_app::server::serve_stdio(Some(home.to_owned()), mode))?;
            return Ok(());
        }
        runtime.block_on(supervise(home))
    }
}

/// `cox attach --stdio`: never starts a supervisor, so it fails, naming the
/// socket, when none runs.
pub fn attach(home: &Path) -> anyhow::Result<()> {
    #[cfg(not(unix))]
    {
        let _ = home;
        anyhow::bail!("cox attach is not supported yet on this platform");
    }
    #[cfg(unix)]
    {
        tokio::runtime::Runtime::new()?.block_on(attach::stdio(home))?;
        Ok(())
    }
}

#[cfg(unix)]
async fn supervise(home: &Path) -> anyhow::Result<()> {
    // Bind first: a second `cox serve` is refused by the lock before it
    // spends a login shell on nothing.
    let bound = socket::bind(home)?;
    // Workers inherit this environment, so they do not run the login shell
    // again (D3). A failed read only leaves the process's own environment.
    if let Some(warning) = cox_app::app::load_login_env().await {
        tracing::warn!(%warning, "could not read the login shell environment");
    }
    let app = cox_app::server::workspace_app(Some(home.to_owned()))?;
    // `--home` rather than a bare `serve --worker`, so a worker keeps the
    // home this supervisor was given on the command line.
    let mut options = Options::worker(std::env::current_exe()?);
    options.args.extend(["--home".into(), home.into()]);
    eprintln!("cox serve: listening on {}", bound.path.display());
    let killed = router::serve(options, Arc::new(AppWorkspace(app)), bound, terminated()).await?;
    if killed > 0 {
        eprintln!("cox serve: {killed} worker(s) did not end in time and were killed");
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn the_workspace_impl_answers_workspace_calls_and_refuses_the_rest() {
        let home = tempfile::tempdir().expect("tempdir");
        let app = cox_app::server::workspace_app(Some(home.path().to_owned())).expect("app");
        let workspace = AppWorkspace(app);
        let projects = serde_json::json!({"method": "projects", "params": {"limit": 3}});
        assert_eq!(
            workspace.call(&projects),
            Ok(serde_json::json!({"kind": "projects", "value": []}))
        );
        let send = serde_json::json!({"method": "close", "params": {"session": "s"}});
        assert!(workspace.call(&send).is_err());
    }
}
