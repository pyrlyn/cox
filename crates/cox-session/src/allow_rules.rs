// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The `AllowRuleStore` every surface installs (T69.2): an `AllowForSession`
//! answer that `permissions.allow_for_session_persists` keeps lands in the
//! user config's `permissions.allow` through `cox-config`'s one writer.
//! Separate from `cox-core` because that crate does no file I/O, and from
//! `cox-config` because the trait is the core's.

use std::path::PathBuf;

use cox_protocol::CoreError;
use cox_protocol::traits::AllowRuleStore;

/// Appends to the config file at `path`; the path is a field so a test can
/// point at a scratch file instead of the real `~/.cox`.
#[derive(Debug, Clone)]
pub struct UserConfigRules {
    path: PathBuf,
}

impl UserConfigRules {
    /// The writer for the user config the next session would load.
    pub fn user() -> Self {
        Self::at(cox_config::load::user_config_path())
    }

    /// The writer for the config file at `path`.
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }
}

impl AllowRuleStore for UserConfigRules {
    fn append_allow(&self, rule: &str) -> Result<bool, CoreError> {
        cox_config::cmd::append_allow_rule_in(&self.path, rule).map_err(|e| CoreError::Config {
            key: "permissions.allow".into(),
            message: e.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_for_session_rule_lands_once_in_the_given_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let store = UserConfigRules::at(path.clone());
        assert!(store.append_allow("Bash(git status:*)").expect("first"));
        assert!(!store.append_allow("Bash(git status:*)").expect("second"));
        let written = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(
            written.matches("Bash(git status:*)").count(),
            1,
            "{written}"
        );
    }

    #[test]
    fn allow_for_session_unwritable_target_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A directory where the file should be: the read fails.
        let store = UserConfigRules::at(dir.path().to_path_buf());
        assert!(store.append_allow("Bash(ls:*)").is_err());
    }
}
