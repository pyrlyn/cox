// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! What the toolbar's model popover offers (DT§5.1 "model chip", T37.22.6):
//! each tier's provider's models by the id `/model` takes, grouped into the
//! popover's sections (T58.4.7), and which providers can answer at all
//! (A110). Separate
//! from Settings, which edits the config, because these only read it to
//! choose where the next turn goes.

use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use cox_protocol::Config;
use cox_protocol::config::Transport;
use cox_protocol::types::{Effort, Tier};
use cox_session::doctor::check_api_keys_in;

use crate::app::{App, AppError};

/// The tiers in the order the popover lists them: the one turns run on,
/// then the ones a person switches to for a hard or a cheap turn.
const TIERS: [Tier; 3] = [Tier::Code, Tier::Think, Tier::Cheap];

/// How long a local server may take to accept a connection before the
/// footer counts it as down: a loopback connect answers in well under this.
const PROBE_TIMEOUT: Duration = Duration::from_millis(300);

/// One model a tier can switch to, as `/model <tier> <id>` names it.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelChoice {
    pub tier: Tier,
    /// The `[providers.<name>]` section the tier calls.
    pub provider: String,
    /// The id sent on the wire.
    pub id: String,
    /// What the catalog calls it (`Claude Sonnet 5`, A111); `None` when it
    /// has no name, and the popover shows the id.
    pub display_name: Option<String>,
    /// `display_name` as the desktop shows it, `Sonnet 5` (A129,
    /// `status::shorten`); `None` with it.
    pub short_name: Option<String>,
    /// The efforts it takes; empty means any.
    pub efforts: Vec<Effort>,
    /// Its window in tokens, when the config knows it.
    pub context_window: Option<u32>,
}

/// Every tier's models, the tier's configured one first, also when its
/// section does not list it (LM Studio lists none).
pub fn choices(config: &Config) -> Vec<ModelChoice> {
    let names = crate::status::model_names(config);
    let mut out = Vec::new();
    for tier in TIERS {
        let t = config.tiers.get(tier);
        let listed = config.providers.models_for(&t.provider);
        let choice = |id: &str, efforts: Vec<Effort>, context_window| ModelChoice {
            tier,
            provider: t.provider.clone(),
            id: id.to_owned(),
            display_name: names.get(id).cloned(),
            short_name: names.short(id).cloned(),
            efforts,
            context_window,
        };
        match listed.iter().find(|m| m.id == t.model) {
            Some(m) => out.push(choice(&m.id, m.efforts.clone(), Some(m.context_window))),
            None if !t.model.is_empty() => out.push(choice(&t.model, Vec::new(), None)),
            None => {}
        }
        out.extend(
            listed
                .iter()
                .filter(|m| m.id != t.model)
                .map(|m| choice(&m.id, m.efforts.clone(), Some(m.context_window))),
        );
    }
    out
}

/// One section of the model popover: a tier's models not listed earlier.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelSection {
    pub tier: Tier,
    /// `Code`, `Think`, `Cheap`.
    pub title: String,
    pub models: Vec<MenuModel>,
}

/// A popover row; which one the session runs on is the client's to mark.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuModel {
    /// The id the switch sends.
    pub id: String,
    /// What the catalog calls it; `None` when it has no name.
    pub display_name: Option<String>,
    /// What the row shows, `Sonnet 5` (A129); `None` with `display_name`,
    /// and the row shows the id.
    pub short_name: Option<String>,
    /// `low · high`: the efforts it takes; empty when it takes any.
    pub efforts: String,
}

/// The popover's sections: one per tier in first-listed order, a model a
/// tier already lists left out of a later one's, so with every tier on one
/// provider the menu is one list.
pub fn menu(choices: Vec<ModelChoice>) -> Vec<ModelSection> {
    let mut listed = std::collections::HashSet::new();
    let mut out: Vec<ModelSection> = Vec::new();
    for choice in choices {
        if !listed.insert(choice.id.clone()) {
            continue;
        }
        let model = MenuModel {
            efforts: choice
                .efforts
                .iter()
                .map(|e| e.name())
                .collect::<Vec<_>>()
                .join(" · "),
            id: choice.id,
            display_name: choice.display_name,
            short_name: choice.short_name,
        };
        match out.iter_mut().find(|s| s.tier == choice.tier) {
            Some(section) => section.models.push(model),
            None => out.push(ModelSection {
                tier: choice.tier,
                title: title(choice.tier).to_owned(),
                models: vec![model],
            }),
        }
    }
    out
}

fn title(tier: Tier) -> &'static str {
    match tier {
        Tier::Code => "Code",
        Tier::Think => "Think",
        Tier::Cheap => "Cheap",
    }
}

/// Every `[providers.<name>]` section with its transport, by name.
fn sections(config: &Config) -> Vec<(String, Transport)> {
    let p = &config.providers;
    let mut out = vec![
        ("anthropic".to_owned(), p.anthropic.transport()),
        ("openai".to_owned(), p.openai.transport()),
        ("local".to_owned(), p.local.transport()),
        ("lmstudio".to_owned(), p.lmstudio.transport()),
        ("typesafe".to_owned(), p.typesafe.transport()),
    ];
    out.extend(p.custom.iter().map(|(n, c)| (n.clone(), c.transport())));
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// How a section proves it can answer: a key, or a server on this machine.
#[derive(Debug, PartialEq)]
enum Reach {
    Key(bool),
    Server(String, u16),
}

/// A section on this machine's loopback is usable while its server runs,
/// with or without a key (a local server rarely needs one); any other is
/// usable once `cox doctor`'s key check finds its key.
fn reach(
    config: &Config,
    section: &str,
    base_url: &str,
    keys: &dyn Fn(&str) -> Option<String>,
) -> Reach {
    if let Some((host, port)) = loopback(base_url) {
        return Reach::Server(host, port);
    }
    let mut asked = config.clone();
    asked.tiers.code.provider = section.to_owned();
    Reach::Key(check_api_keys_in(&asked, keys).status == "ok")
}

/// `http://localhost:11434/v1` → `("localhost", 11434)`; `None` for a host
/// that is not this machine.
fn loopback(url: &str) -> Option<(String, u16)> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let (host, port) = match authority.strip_prefix('[') {
        Some(v6) => {
            let (host, tail) = v6.split_once(']')?;
            (host, tail.strip_prefix(':'))
        }
        None => match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    };
    let local = host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
    if !local {
        return None;
    }
    let port = match port {
        Some(p) => p.parse().ok()?,
        None if scheme.eq_ignore_ascii_case("https") => 443,
        None => 80,
    };
    Some((host.to_owned(), port))
}

/// Whether something accepts connections at `host:port` within the probe's
/// timeout; a closed loopback port refuses at once.
async fn listening(host: &str, port: u16) -> bool {
    let connect = tokio::net::TcpStream::connect((host, port));
    matches!(
        tokio::time::timeout(PROBE_TIMEOUT, connect).await,
        Ok(Ok(_))
    )
}

/// The sections a turn could run on now (A110): a key found, or a local
/// server listening. Sorted by name.
pub async fn usable(
    config: &Config,
    keys: &(dyn Fn(&str) -> Option<String> + Sync),
) -> Vec<String> {
    let reaches: Vec<(String, Reach)> = sections(config)
        .into_iter()
        .map(|(name, t)| {
            let r = reach(config, &name, &t.base_url, keys);
            (name, r)
        })
        .collect();
    let mut out = Vec::new();
    for (name, r) in reaches {
        let up = match r {
            Reach::Key(found) => found,
            Reach::Server(host, port) => listening(&host, port).await,
        };
        if up {
            out.push(name);
        }
    }
    out
}

impl App {
    /// The model popover's rows for a session in `cwd`.
    pub fn models(&self, cwd: &Path) -> Result<Vec<ModelChoice>, AppError> {
        Ok(choices(&self.config(cwd)?))
    }

    /// The model popover's sections for a session in `cwd` (T58.4.7).
    pub fn model_menu(&self, cwd: &Path) -> Result<Vec<ModelSection>, AppError> {
        Ok(menu(choices(&self.config(cwd)?)))
    }

    /// The providers a turn in `cwd` could run on now (A110), each key
    /// looked up as the checklist does; probes local servers, so call it
    /// off the main thread.
    pub async fn usable_providers(&self, cwd: &Path) -> Result<Vec<String>, AppError> {
        let config = self.config(cwd)?;
        let host = &self.host;
        let keys = |section: &str| host.secret(section);
        Ok(usable(&config, &keys).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tier_lists_its_providers_models_and_an_unlisted_configured_one_first() {
        let mut config = Config::default();
        config.tiers.code.provider = "anthropic".into();
        config.tiers.code.model = "claude-custom-9".into();
        config.providers.anthropic.models = vec![cox_protocol::config::ProviderModel {
            id: "claude-sonnet-5".into(),
            context_window: 1_000_000,
            efforts: vec![Effort::Low, Effort::High],
            ..Default::default()
        }];
        let code: Vec<(String, Option<u32>, Option<String>)> = choices(&config)
            .into_iter()
            .filter(|c| c.tier == Tier::Code)
            .map(|c| (c.id, c.context_window, c.display_name))
            .collect();
        // A config entry without a name keeps the catalog's (A111); an id
        // the catalog does not know has none.
        assert_eq!(
            code,
            [
                ("claude-custom-9".into(), None, None),
                (
                    "claude-sonnet-5".into(),
                    Some(1_000_000),
                    Some("Claude Sonnet 5".into())
                ),
            ]
        );
        let tiers: Vec<Tier> = choices(&config).iter().map(|c| c.tier).collect();
        assert_eq!(tiers.first(), Some(&Tier::Code));
        let rows = &menu(choices(&config))[0].models;
        let shown: Vec<_> = rows.iter().map(|m| m.short_name.as_deref()).collect();
        assert_eq!(shown, [None, Some("Sonnet 5")], "a menu row's short name");
    }

    fn choice(tier: Tier, id: &str, efforts: Vec<Effort>) -> ModelChoice {
        ModelChoice {
            tier,
            provider: "anthropic".into(),
            id: id.into(),
            display_name: None,
            short_name: None,
            efforts,
            context_window: None,
        }
    }

    fn ids(sections: &[ModelSection]) -> Vec<(String, Vec<String>)> {
        sections
            .iter()
            .map(|s| {
                (
                    s.title.clone(),
                    s.models.iter().map(|m| m.id.clone()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn a_model_is_listed_once_across_tiers() {
        let sections = menu(vec![
            choice(Tier::Code, "sonnet", vec![Effort::Low, Effort::High]),
            choice(Tier::Code, "opus", vec![]),
            choice(Tier::Think, "opus", vec![]),
            choice(Tier::Think, "fable", vec![]),
            choice(Tier::Cheap, "sonnet", vec![]),
        ]);
        assert_eq!(
            ids(&sections),
            [
                ("Code".into(), vec!["sonnet".into(), "opus".into()]),
                ("Think".into(), vec!["fable".into()]),
            ],
            "a tier whose models are all listed earlier has no section"
        );
        assert_eq!(sections[0].models[0].efforts, "low · high");
        assert_eq!(sections[0].models[1].efforts, "", "empty takes any");
    }

    #[test]
    fn tiers_keep_their_first_listed_order() {
        let sections = menu(vec![
            choice(Tier::Cheap, "haiku", vec![]),
            choice(Tier::Code, "sonnet", vec![]),
            choice(Tier::Cheap, "mini", vec![]),
        ]);
        assert_eq!(
            ids(&sections),
            [
                ("Cheap".into(), vec!["haiku".into(), "mini".into()]),
                ("Code".into(), vec!["sonnet".into()]),
            ]
        );
    }

    #[test]
    fn only_a_loopback_url_is_a_local_server() {
        assert_eq!(
            loopback("http://localhost:11434/v1"),
            Some(("localhost".into(), 11434))
        );
        assert_eq!(loopback("http://127.0.0.1"), Some(("127.0.0.1".into(), 80)));
        assert_eq!(loopback("https://[::1]:8443/x"), Some(("::1".into(), 8443)));
        assert_eq!(loopback("https://api.anthropic.com"), None);
        assert_eq!(loopback("http://10.0.0.5:1234"), None);
    }

    /// Two keyed sections (one found, one not) and two local servers (one
    /// listening, one not), with every other section on a host that is not
    /// this machine and has no key.
    #[tokio::test]
    async fn the_footer_counts_a_found_key_and_a_listening_server_only() {
        let open = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let up = open.local_addr().expect("addr").port();
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let down = closed.local_addr().expect("addr").port();
        drop(closed);

        let mut config = Config::default();
        let unset = "COX_MODELS_TEST_UNSET_KEY";
        config.providers.anthropic.api_key_env = unset.into();
        config.providers.openai.api_key_env = unset.into();
        config.providers.typesafe.api_key_env = unset.into();
        config.providers.lmstudio.api_key_env = unset.into();
        config.providers.local.base_url = format!("http://127.0.0.1:{up}/v1");
        config.providers.lmstudio.base_url = format!("http://localhost:{down}");
        config.providers.openai.base_url = "https://api.openai.com/v1".into();
        config.providers.typesafe.base_url = "https://api.typesafe.ai".into();
        let keys = |section: &str| (section == "anthropic").then(|| "sk-test".to_owned());
        assert_eq!(usable(&config, &keys).await, ["anthropic", "local"]);
    }
}
