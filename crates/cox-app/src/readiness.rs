//! Whether a prompt may be sent at all (DT§5.3, A139): the session's
//! code-tier provider must be set and in the usable list (A110). One rule in
//! Rust, so the macOS app, a Windows or Linux client and `LiveSession::send`
//! agree and a client that forgets the gate still cannot start a turn.
//! Separate from `models`, which lists what could answer; this decides
//! about the one provider a turn would go to.

use std::path::Path;

use cox_protocol::Config;

use crate::app::{App, AppError};
use crate::models::{loopback, sections, usable};

/// Why a turn can or cannot start now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readiness {
    Ready,
    /// `tiers.code.provider` is empty or names no `[providers.<name>]` section.
    NoProvider,
    /// A keyed provider whose key was not found (env var, then the host's store).
    NoKey {
        provider: String,
    },
    /// A provider on this machine whose server does not accept connections.
    Unreachable {
        provider: String,
    },
}

impl Readiness {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }

    /// The text a client shows where it disables sending (DT§5.3); `None`
    /// when ready.
    // why: cox-app has no cox-i18n dependency yet, so the texts are English
    // here; a client localizes by matching the variant.
    pub fn message(&self) -> Option<String> {
        match self {
            Self::Ready => None,
            Self::NoProvider => {
                Some("No provider is set for the code tier. Choose one in Settings.".into())
            }
            Self::NoKey { provider } => Some(format!(
                "No API key for {provider}. Add one in Settings, or pick another provider."
            )),
            Self::Unreachable { provider } => Some(format!(
                "{provider} is not running on this machine. Start its server, or pick another provider."
            )),
        }
    }
}

/// `usable` is [`crate::models::usable`]'s list for `config`. A provider that
/// is not in it is `Unreachable` when its URL is on this machine's loopback
/// (a server that is down) and `NoKey` otherwise.
pub fn readiness(config: &Config, usable: &[String]) -> Readiness {
    let provider = config.tiers.code.provider.as_str();
    let Some((_, transport)) = sections(config).into_iter().find(|(n, _)| n == provider) else {
        return Readiness::NoProvider;
    };
    let provider = provider.to_owned();
    if usable.contains(&provider) {
        Readiness::Ready
    } else if loopback(&transport.base_url).is_some() {
        Readiness::Unreachable { provider }
    } else {
        Readiness::NoKey { provider }
    }
}

/// A test double (`COX_PROVIDER`) answers instead of any provider, so there
/// is no key or server to look for; `cox-session` skips its own provider
/// checks on the same variable.
fn test_double() -> bool {
    std::env::var_os("COX_PROVIDER").is_some_and(|v| !v.is_empty())
}

impl App {
    /// Whether a turn in `cwd` may start now. Probes on every call, so a key
    /// added in Settings or a server just started counts at once; call it off
    /// the main thread.
    pub async fn readiness(&self, cwd: &Path) -> Result<Readiness, AppError> {
        Ok(self.readiness_of(&self.config(cwd)?).await)
    }

    /// [`App::readiness`] for a session's own `config`: one reopened on a
    /// picked provider (T60.3) runs on it, not on the default.
    pub(crate) async fn readiness_of(&self, config: &Config) -> Readiness {
        if test_double() {
            return Readiness::Ready;
        }
        let host = &self.host;
        let keys = |section: &str| host.secret(section);
        readiness(config, &usable(config, &keys).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_provider(name: &str) -> Config {
        let mut config = Config::default();
        config.tiers.code.provider = name.into();
        config
    }

    #[test]
    fn a_keyed_provider_without_a_key_is_no_key() {
        let config = with_provider("anthropic");
        assert_eq!(
            readiness(&config, &[]),
            Readiness::NoKey {
                provider: "anthropic".into()
            }
        );
        assert_eq!(readiness(&config, &["anthropic".into()]), Readiness::Ready);
    }

    #[test]
    fn a_loopback_provider_that_is_not_listening_is_unreachable() {
        let config = with_provider("local");
        assert_eq!(
            readiness(&config, &["anthropic".into()]),
            Readiness::Unreachable {
                provider: "local".into()
            }
        );
        assert!(readiness(&config, &["local".into()]).is_ready());
    }

    #[test]
    fn an_empty_or_unknown_provider_is_no_provider() {
        for name in ["", "nonesuch"] {
            let usable = [name.to_owned()];
            assert_eq!(
                readiness(&with_provider(name), &usable),
                Readiness::NoProvider
            );
        }
        let mut config = with_provider("deepseek");
        config.providers.custom.insert(
            "deepseek".into(),
            cox_protocol::config::CompatibleProviderConfig::default(),
        );
        assert_ne!(readiness(&config, &[]), Readiness::NoProvider);
    }

    #[test]
    fn every_blocked_outcome_has_a_text_naming_the_provider() {
        assert_eq!(Readiness::Ready.message(), None);
        let texts = [
            Readiness::NoProvider,
            Readiness::NoKey {
                provider: "openai".into(),
            },
            Readiness::Unreachable {
                provider: "lmstudio".into(),
            },
        ]
        .map(|r| r.message().expect("a text"));
        assert!(texts[1].contains("openai") && texts[2].contains("lmstudio"));
    }
}
