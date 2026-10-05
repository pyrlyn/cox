// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The first-run checklist (DT§5.8, T37.31): whether cox can work here —
//! the code tier's provider key, `git`, the sandbox and the login shell's
//! environment — each with the one line that says what is missing. The
//! first three are `cox doctor`'s own checks (`cox_session::doctor`), so
//! the app and the CLI never disagree; the shell row is the app's alone,
//! since only the app reads a login shell at launch (DT§4.8).

use std::path::Path;
use std::sync::OnceLock;

use cox_protocol::Config;
use cox_session::doctor::{CheckResult, check_api_keys_in, check_git, check_sandbox};

use crate::app::{App, AppError};

/// What `load_login_env` reported at launch: why it fell back, if it did.
/// Unset until it has run.
static LOGIN_ENV: OnceLock<Option<String>> = OnceLock::new();

/// Called once by `load_login_env`; a second launch-time load is ignored.
pub(crate) fn remember_login(warning: Option<&str>) {
    let _ = LOGIN_ENV.set(warning.map(str::to_owned));
}

/// Which check a row reports; the app names it and picks its fix button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckId {
    ProviderKey,
    Git,
    Sandbox,
    ShellEnv,
}

/// `cox doctor`'s `ok`, `warn` and `fail`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

/// One checklist row: what was checked, how it went, and what is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRow {
    pub id: CheckId,
    pub status: CheckStatus,
    pub detail: String,
}

/// The rows in the order DT§5.8 lists them. `keys` is where the app keeps
/// provider keys (the Keychain, through the host); `login` is what the
/// login-shell load reported, `None` before it ran.
pub fn checklist(
    config: &Config,
    keys: &dyn Fn(&str) -> Option<String>,
    login: Option<Option<&str>>,
) -> Vec<CheckRow> {
    vec![
        row(CheckId::ProviderKey, check_api_keys_in(config, keys)),
        row(CheckId::Git, check_git()),
        row(CheckId::Sandbox, check_sandbox()),
        shell_row(login),
    ]
}

fn row(id: CheckId, result: CheckResult) -> CheckRow {
    let status = match result.status.as_str() {
        "ok" => CheckStatus::Ok,
        "warn" => CheckStatus::Warn,
        _ => CheckStatus::Fail,
    };
    CheckRow {
        id,
        status,
        detail: result.detail,
    }
}

/// A fallback is a warning, not a failure: sessions still run on the
/// inherited environment, they just may not find `cargo` or an env key.
fn shell_row(login: Option<Option<&str>>) -> CheckRow {
    let (status, detail) = match login {
        None => (CheckStatus::Warn, "the login shell has not been read yet"),
        Some(None) => (CheckStatus::Ok, "PATH and variables from your login shell"),
        Some(Some(warning)) => (CheckStatus::Warn, warning),
    };
    CheckRow {
        id: CheckId::ShellEnv,
        status,
        detail: detail.to_owned(),
    }
}

impl App {
    /// The checklist for a session in `cwd`, whose config names the
    /// provider whose key is checked.
    pub fn checklist(&self, cwd: &Path) -> Result<Vec<CheckRow>, AppError> {
        let config = self.config(cwd)?;
        let host = &self.host;
        let keys = |section: &str| host.secret(section);
        Ok(checklist(
            &config,
            &keys,
            LOGIN_ENV.get().map(Option::as_deref),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Anthropic with an env var nobody sets, so only `keys` can answer.
    fn anthropic() -> Config {
        let mut config = Config::default();
        config.tiers.code.provider = "anthropic".into();
        config.providers.anthropic.api_key_env = "COX_ONBOARDING_TEST_UNSET_KEY".into();
        config
    }

    #[test]
    fn a_missing_provider_key_fails_and_names_what_is_missing() {
        let rows = checklist(&anthropic(), &|_| None, Some(None));
        let ids: Vec<CheckId> = rows.iter().map(|r| r.id).collect();
        assert_eq!(
            ids,
            [
                CheckId::ProviderKey,
                CheckId::Git,
                CheckId::Sandbox,
                CheckId::ShellEnv
            ]
        );
        assert_eq!(rows[0].status, CheckStatus::Fail);
        assert!(
            rows[0].detail.contains("COX_ONBOARDING_TEST_UNSET_KEY"),
            "{}",
            rows[0].detail
        );
        assert_eq!(rows[3].status, CheckStatus::Ok);
    }

    #[test]
    fn a_stored_key_passes_and_a_shell_fallback_warns_with_its_reason() {
        let keys = |section: &str| (section == "anthropic").then(|| "sk-test".to_owned());
        let rows = checklist(&anthropic(), &keys, Some(Some("zsh timed out")));
        assert_eq!(rows[0].status, CheckStatus::Ok);
        assert_eq!(
            (rows[3].status, rows[3].detail.as_str()),
            (CheckStatus::Warn, "zsh timed out")
        );
    }

    #[test]
    fn the_shell_row_warns_before_the_login_shell_is_read() {
        assert_eq!(shell_row(None).status, CheckStatus::Warn);
    }
}
