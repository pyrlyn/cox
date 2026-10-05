// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The checks `cox doctor` and the desktop app's first-run checklist
//! (DT§5.8, T37.31) share: the code tier's provider key, the sandbox
//! backend and `git` on PATH. Here rather than in `crates/cox` because
//! `cox-app` cannot depend on the CLI, and the two surfaces must never
//! disagree about whether cox will work.

/// One check result.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckResult {
    pub check: String,
    pub status: String,
    pub detail: String,
    pub fix: String,
}

impl CheckResult {
    /// Create an `ok` result.
    pub fn ok(check: &str, detail: String) -> Self {
        CheckResult {
            check: check.to_string(),
            status: "ok".to_string(),
            detail,
            fix: String::new(),
        }
    }

    /// Create a `warn` result.
    pub fn warn(check: &str, detail: String, fix: String) -> Self {
        CheckResult {
            check: check.to_string(),
            status: "warn".to_string(),
            detail,
            fix,
        }
    }

    /// Create a `fail` result.
    pub fn fail(check: &str, detail: String, fix: String) -> Self {
        CheckResult {
            check: check.to_string(),
            status: "fail".to_string(),
            detail,
            fix,
        }
    }
}

/// What the `code` tier's provider needs for a key (T30.21).
#[derive(Debug, PartialEq)]
enum KeyRequirement<'a> {
    /// The section's name, the env var its `api_key_env` names, and whether
    /// a missing key is fatal: Anthropic and Jev fail without one;
    /// OpenAI-shaped sections run keyless against a local server.
    Key(&'a str, &'a str, bool),
    /// `local` never sends a key.
    None,
    /// No `[providers.<name>]` section: the session refuses to start, so
    /// doctor must not call this "needs no key".
    UnknownProvider(&'a str),
}

fn key_requirement(config: &cox_protocol::Config) -> KeyRequirement<'_> {
    let section = config.tiers.code.provider.as_str();
    let p = &config.providers;
    match section {
        "anthropic" => KeyRequirement::Key(section, p.anthropic.api_key_env.as_str(), true),
        "typesafe" => KeyRequirement::Key(section, p.typesafe.api_key_env.as_str(), true),
        "openai" => KeyRequirement::Key(section, p.openai.api_key_env.as_str(), false),
        "local" => KeyRequirement::None,
        // T30.15: same optional-key shape as `openai` — LM Studio runs
        // keyless unless "Require Authentication" is on.
        "lmstudio" => KeyRequirement::Key(section, p.lmstudio.api_key_env.as_str(), false),
        _ => match p.custom.get(section) {
            Some(c) => KeyRequirement::Key(section, c.api_key_env.as_str(), false),
            None => KeyRequirement::UnknownProvider(section),
        },
    }
}

/// [`check_api_keys`]'s body with the credential lookup injected, so a test
/// can exercise every branch (unknown provider, keyless, found, missing)
/// without ever touching the real keyring (A49, T30.28).
fn check_api_keys_with(
    config: &cox_protocol::Config,
    resolve: impl FnOnce(&str, &str) -> Result<String, cox_protocol::errors::ProviderError>,
) -> CheckResult {
    let (section, env_var, required) = match key_requirement(config) {
        KeyRequirement::Key(section, env_var, required) => (section, env_var, required),
        KeyRequirement::None => {
            return CheckResult::ok(
                "API keys",
                "the code tier's provider needs no key".to_string(),
            );
        }
        KeyRequirement::UnknownProvider(section) => {
            return CheckResult::fail(
                "API keys",
                format!("tiers.code.provider `{section}` has no [providers.{section}] section"),
                format!(
                    "add [providers.{section}] or point tiers.code.provider at a configured provider"
                ),
            );
        }
    };
    if resolve(env_var, section).is_ok() {
        return CheckResult::ok("API keys", format!("{section} key found"));
    }
    let detail = format!("{env_var} is not set and keyring entry 'cox/{section}' not found");
    let fix = format!(
        "set {env_var} or run `security add-generic-password -s cox -a {section} -w` (macOS) or your platform's keyring equivalent"
    );
    if required {
        CheckResult::fail("API keys", detail, fix)
    } else {
        CheckResult::warn(
            "API keys",
            format!("{detail}; requests go out without a key"),
            fix,
        )
    }
}

/// Resolves the key exactly as the provider will (`cox_provider::http::resolve_key`:
/// the section's env var, then keyring `cox/<section>`), so doctor and the
/// session never disagree about whether a key exists.
pub fn check_api_keys(config: &cox_protocol::Config) -> CheckResult {
    check_api_keys_with(config, cox_provider::http::resolve_key)
}

/// [`check_api_keys`] with the stored key looked up in `keys` rather than
/// the OS keyring, as a session opened with `open_with_keys` does (T37.14):
/// the macOS app's Keychain answers through Swift. The env var still wins.
pub fn check_api_keys_in(
    config: &cox_protocol::Config,
    keys: &dyn Fn(&str) -> Option<String>,
) -> CheckResult {
    check_api_keys_with(config, |var, section| {
        cox_provider::http::resolve_key_with(var, section, keys)
    })
}

pub fn check_sandbox() -> CheckResult {
    match cox_tools::sandbox::backend(cox_protocol::LinuxBackend::Auto) {
        Some(backend) => CheckResult::ok("sandbox", backend.name().to_string()),
        None => CheckResult::warn(
            "sandbox",
            "none: shell commands run unconfined".to_string(),
            match std::env::consts::OS {
                "macos" => "sandbox-exec is part of macOS; check your installation".to_string(),
                "linux" => {
                    "install bubblewrap: apt install bubblewrap (Debian/Ubuntu) or equivalent"
                        .to_string()
                }
                _ => "sandbox is not supported on this platform".to_string(),
            },
        ),
    }
}

pub fn check_git() -> CheckResult {
    match std::process::Command::new("git").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            CheckResult::ok("git", version.trim().to_string())
        }
        _ => CheckResult::fail(
            "git",
            "git not found on PATH".to_string(),
            "install git from https://git-scm.com/".to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_checks_the_key_the_code_tier_provider_names() {
        let mut config = cox_protocol::Config::default();
        config.tiers.code.provider = "anthropic".into();
        config.providers.anthropic.api_key_env = "MY_ANTHROPIC_KEY".into();
        assert_eq!(
            key_requirement(&config),
            KeyRequirement::Key("anthropic", "MY_ANTHROPIC_KEY", true)
        );
        config.tiers.code.provider = "local".into();
        assert_eq!(key_requirement(&config), KeyRequirement::None);
        config.tiers.code.provider = "lmstudio".into();
        config.providers.lmstudio.api_key_env = "LM_API_TOKEN".into();
        assert_eq!(
            key_requirement(&config),
            KeyRequirement::Key("lmstudio", "LM_API_TOKEN", false)
        );
        config.tiers.code.provider = "deepseek".into();
        config.providers.custom.insert(
            "deepseek".into(),
            cox_protocol::config::CompatibleProviderConfig {
                api_key_env: "DEEPSEEK_API_KEY".into(),
                ..Default::default()
            },
        );
        assert_eq!(
            key_requirement(&config),
            KeyRequirement::Key("deepseek", "DEEPSEEK_API_KEY", false)
        );
    }

    #[test]
    fn doctor_fails_when_the_code_tier_names_an_unknown_provider() {
        // The session refuses this config ("unknown provider"); doctor must
        // not report it as a provider that needs no key. Returns before any
        // keyring lookup — enforced here by a lookup that panics if called
        // (A49, T30.28).
        let mut config = cox_protocol::Config::default();
        config.tiers.code.provider = "nosuch".into();
        let result = check_api_keys_with(&config, |_, _| {
            panic!("an unknown provider must fail before any key is resolved")
        });
        assert_eq!(result.status, "fail");
        assert!(result.detail.contains("[providers.nosuch]"));
    }

    #[test]
    fn doctor_warns_not_fails_when_a_keyless_section_has_no_key() {
        // A section name no keyring holds and an env var nobody sets —
        // simulated with an injected lookup rather than the real keyring
        // (A49, T30.28).
        let section = "cox-doctor-test-keyless";
        let mut config = cox_protocol::Config::default();
        config.tiers.code.provider = section.into();
        config.providers.custom.insert(
            section.into(),
            cox_protocol::config::CompatibleProviderConfig {
                api_key_env: "COX_DOCTOR_TEST_UNSET_KEY".into(),
                ..Default::default()
            },
        );
        let result = check_api_keys_with(&config, |_, _| {
            Err(cox_protocol::errors::ProviderError::Auth)
        });
        assert_eq!(result.status, "warn");
        // The keyring hint names service `cox`, account `<section>` — the
        // order `keyring::Entry::new("cox", section)` reads. The fix text
        // stays out of the assertion message: it is the failure output
        // CodeQL treats as a log, and a key check's text is sensitive.
        assert!(result.fix.contains(&format!("-s cox -a {section}")));
    }

    #[test]
    fn the_app_finds_the_provider_key_in_its_own_store() {
        // An env var nobody sets, so only the injected store can answer
        // (A49: the real keychain is never read).
        let mut config = cox_protocol::Config::default();
        config.tiers.code.provider = "anthropic".into();
        config.providers.anthropic.api_key_env = "COX_DOCTOR_TEST_UNSET_ANTHROPIC".into();
        let stored = |section: &str| (section == "anthropic").then(|| "sk-test".to_string());
        assert_eq!(check_api_keys_in(&config, &stored).status, "ok");
        let result = check_api_keys_in(&config, &|_| None);
        assert_eq!(
            result.status, "fail",
            "expected fail status when no stored key is available"
        );
        assert!(result.detail.contains("cox/anthropic"));
    }
}
