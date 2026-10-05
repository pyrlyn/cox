//! The plugin contract shared by the host (`cox-plugin`), the guest SDK and
//! `cox_protocol::plugin`: the `plugin.toml` manifest, the ABI payloads and
//! the widget tree. A crate of its own because the
//! guest SDK builds it for `wasm32-unknown-unknown`, so it depends on no
//! workspace crate and does no I/O (plan.md §1.1, A52).
//!
//! - [`manifest`] — `PluginManifest` and its validation (PL§2); the schema is
//!   committed as `docs/plugin.schema.json`.
//! - [`abi`] — the ABI v1 payloads (PL§4); the schema is committed as
//!   `docs/plugin-abi.schema.json`.
//! - [`ui`] — the `Widget` tree `cox_render` returns (PL§8); its schema is
//!   part of `docs/plugin-abi.schema.json`.

pub mod abi;
pub mod manifest;
pub mod ui;

pub use abi::{
    AbiError, Advice, Answer, CommandDecl, CommandIn, CommandOut, DecidePoint, Effects, EventBatch,
    HookCall, HttpReq, HttpResp, InitIn, InitOut, KeyDecl, ModelCall, NoticeLevel, PluginNotice,
    ProviderCall, Question, RenderIn, RenderItemIn, SessionInfo, Slot, ToolCallIn,
};

pub use manifest::{
    API_MAJOR, AgentDecl, AgentMode, Capabilities, CloudAgentDecl, CloudBackend, ExternalAgentDecl,
    FsCaps, Limits, ManifestError, McpDecl, ModelDecl, ModelTier, PluginManifest, PriceDecl,
    ProviderApi, ProviderAuth, ProviderDecl, UiCaps, is_plugin_id,
};

pub use ui::{StyleToken, Widget};

#[cfg(test)]
mod tests {
    use std::path::Path;

    use schemars::generate::SchemaSettings;
    use schemars::schema_for;
    use serde_json::json;

    use crate::*;

    /// Compares `generated` with the committed `docs/<name>`, writing it on
    /// the first run so `git status` shows it as new for review.
    fn assert_committed(name: &str, generated: &str) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs")
            .join(name);
        match std::fs::read_to_string(&path) {
            Ok(committed) => assert_eq!(
                committed, generated,
                "docs/{name} is stale; regenerate it (see this test) and commit it"
            ),
            Err(_) => std::fs::write(&path, generated).expect("write the schema file"),
        }
    }

    /// Pins the manifest's JSON Schema to `docs/plugin.schema.json`, so a
    /// shape change is a reviewable diff for plugin authors and their
    /// editors instead of a silent break. Same shape as `cox-protocol`'s
    /// `protocol_jsonschema_matches_committed_file`.
    #[test]
    fn plugin_schema_matches_committed_file() {
        let generated = serde_json::to_string_pretty(&schema_for!(PluginManifest))
            .expect("schema serializes")
            + "\n";

        assert_committed("plugin.schema.json", &generated);
    }

    /// Pins every ABI payload's JSON Schema to `docs/plugin-abi.schema.json`
    /// (one `$defs` entry per type), so an ABI change is a reviewable diff
    /// for guest authors in every language, not only Rust.
    #[test]
    fn abi_schema_matches_committed_file() {
        let mut generator = SchemaSettings::draft2020_12().into_generator();
        macro_rules! defs {
            ($($t:ty),* $(,)?) => { $( generator.subschema_for::<$t>(); )* };
        }
        defs!(
            InitIn,
            InitOut,
            EventBatch,
            Effects,
            HookCall,
            ToolCallIn,
            CommandIn,
            CommandOut,
            RenderIn,
            RenderItemIn,
            ProviderCall,
            ModelCall,
            HttpReq,
            HttpResp,
            Question,
            Advice,
            AbiError,
            Widget,
        );
        let schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "cox plugin ABI v1",
            "description": "Payloads of the cox:host/v1 ABI (docs/design/plugins.md §4). \
                A field marked x-cox-protocol carries that cox_protocol type unchanged; \
                see docs/protocol.jsonschema.",
            "$defs": generator.take_definitions(true),
        });
        let generated = serde_json::to_string_pretty(&schema).expect("schema serializes") + "\n";
        assert_committed("plugin-abi.schema.json", &generated);
    }
}
