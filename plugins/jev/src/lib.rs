//! `cox-plugin-jev`: the Jev plugin (T33.40), TypeSafe's "System One"
//! decision model as a `[[provider]] api = "plugin"` section (`plugin.toml`).
//! This crate builds a `wasm32-unknown-unknown` `cdylib` that cox loads with
//! extism, so it lives in the guest workspace `plugins/` (PL§9) rather than
//! the main one.
//!
//! - [`wire`] — the System One request/response types and parser (T33.40.2):
//!   pure, no `extism-pdk`, so it also builds and tests as an `rlib` on the
//!   host target.
//! - [`provider`] — `cox_provider_stream` for the `typesafe` section
//!   (T33.40.3): System One over `cox_http`, pure behind two closures.
//! - [`decide`] — the two-phase `cox_decide`/`cox_decide_resume` (T33.40.4)
//!   with one minimal `risk` question.
//!
//! The full `risk` advisor (T33.40.6) and the `route` advisor (T33.40.9)
//! land in later cards, and `capabilities.decide` with them.

pub mod decide;
pub mod provider;
pub mod wire;

/// The `wasm32-unknown-unknown` exports (PL§4), gated so the pure [`wire`]
/// module above builds and tests on the host target too (T33.40.2's plan:
/// "the extism-pdk glue sits behind `cfg(target_arch = "wasm32")`").
#[cfg(target_arch = "wasm32")]
mod guest {
    use std::sync::atomic::AtomicBool;

    use cox_plugin_sdk::{
        Advice, DecideOut, DecideResume, InitIn, InitOut, NoticeLevel, ProviderCall, Question,
        SdkError, Value,
    };

    use crate::provider::{self, StreamError};

    /// Whether this instance already warned about a tier routed to
    /// `typesafe`: one notice per loaded plugin, not one per request (J13).
    static WARNED: AtomicBool = AtomicBool::new(false);

    /// No contributions beyond the manifest yet: the decision points
    /// (`decide`/`context`) are later cards. An `init` export is still
    /// required first (`docs/plugins.md`).
    fn init(_: InitIn) -> Result<InitOut, SdkError> {
        Ok(InitOut::default())
    }

    fn provider_stream(call: ProviderCall) -> Result<Vec<Value>, StreamError> {
        provider::stream(&call, &WARNED, cox_plugin_sdk::http, |text| {
            // A notice that cannot be shown must not fail the request.
            let _ = cox_plugin_sdk::notify(NoticeLevel::Warn, text);
        })
    }

    fn decide(question: Question) -> Result<DecideOut, SdkError> {
        Ok(crate::decide::decide(&question))
    }

    fn decide_resume(resumed: DecideResume) -> Result<Option<Advice>, SdkError> {
        Ok(crate::decide::resume(&resumed))
    }

    cox_plugin_sdk::register!(
        init => init,
        provider_stream => provider_stream,
        decide_call => decide,
        decide_resume => decide_resume,
    );
}

#[cfg(test)]
mod tests {
    use cox_plugin_sdk::PluginManifest;
    use figment::Figment;
    use figment::providers::{Format, Toml};

    /// This crate's own `plugin.toml`, validated the way `cox-plugin`
    /// validates every package it loads (PL§2).
    const MANIFEST: &str = include_str!("../plugin.toml");

    #[test]
    fn manifest_validates() {
        let manifest: PluginManifest = Figment::from(Toml::string(MANIFEST))
            .extract()
            .expect("plugin.toml parses");
        assert_eq!(manifest.id, "jev");
        assert_eq!(manifest.provider.len(), 1, "the typesafe provider section");
        assert_eq!(manifest.provider[0].name, "typesafe");
        let ids: Vec<_> = manifest.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["jev-1.13.0", "jev-latest"]);
        for model in &manifest.models {
            assert_eq!(model.provider.as_deref(), Some("typesafe"));
            assert_eq!(model.context_window, Some(64_000), "J6, not 128k");
            let price = model.price.as_ref().expect("a price row");
            assert_eq!((price.input, price.output), (0.042, 0.0), "J7");
        }
        assert_eq!(manifest.validate(), Ok(()));
    }
}
