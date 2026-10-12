//! The ABI v1 payloads (PL§4): what every guest export takes and returns and
//! what every `cox:host/v1` host function exchanges, as JSON through extism's
//! `Json<T>`. The committed schema is `docs/plugin-abi.schema.json`.
//!
//! `cox_protocol` types cross the ABI unchanged (`Event`, `HookEvent`,
//! `HookOutcome`, `ToolSpec`, `ToolCall`, `ToolOutput`, `Request`,
//! `ProviderEvent`). This crate must not depend on `cox-protocol` (it builds
//! for wasm32 and `cox-protocol` re-exports it), so those fields are
//! `serde_json::Value` here and `cox-plugin` converts them into the typed
//! protocol values at the boundary. The schema references them by name
//! (`x-cox-protocol`) instead of copying their definitions, so
//! `docs/protocol.jsonschema` stays their one description.
//!
//! No type denies unknown fields: a minor addition (a new field) keeps
//! `api = 1` only because an older peer ignores what it does not know, in
//! both directions (PL§4 "Versioning").

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::manifest::ModelTier;
// The manifest owns `Surface` (PL§15.1). Re-exported so ABI payloads and the
// manifest name the same set of places a plugin can run.
pub use crate::manifest::Surface;

/// `cox_init` input: once per session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InitIn {
    /// The ABI major the host speaks.
    pub api: u32,
    /// The plugin's own id.
    pub plugin_id: String,
    /// The `[plugins.<id>]` config table; the plugin validates it itself.
    pub config: Value,
    /// The session the plugin is loaded into.
    pub session: SessionInfo,
    /// The granted subset of the manifest's `[capabilities]`, same keys.
    /// JSON rather than `Capabilities`, which denies unknown keys: a
    /// capability added in a later minor must not break an older guest.
    pub granted: Value,
    /// Who drives the session, so a plugin returns only its surface's parts.
    /// Absent from a cox older than PL§15; a guest reads absence as
    /// `terminal`, the only surface plugins were written for.
    #[serde(default)]
    pub surface: Option<Surface>,
}

/// The session a plugin is initialised for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionInfo {
    /// The session id.
    pub id: String,
    /// The workspace root.
    pub cwd: String,
}

/// `cox_init` output: what the plugin contributes. Anything not granted is
/// dropped by the host with a notice; every field may be omitted.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct InitOut {
    /// Tool specs, frozen for the session (PL§7).
    #[schemars(extend("items" = { "x-cox-protocol": "ToolSpec" }))]
    pub tools: Vec<Value>,
    /// Slash commands, shown as `/<id>:<name>`.
    pub commands: Vec<CommandDecl>,
    /// Keys, reachable as `<leader> <key>`.
    pub keys: Vec<KeyDecl>,
    /// Status segments the plugin fills (`status.left`, `status.right`).
    pub status: Vec<Slot>,
    /// Panel and overlay slots the plugin fills (`panel`, `overlay`).
    pub panels: Vec<Slot>,
    /// Render targets it serves: `tool:<name>` or `item:assistant_message`.
    pub renderers: Vec<String>,
    /// `Event` serde tags it wants in `cox_on_event`.
    pub subscribe: Vec<String>,
    /// Desktop-only contributions; dropped with a notice on any other
    /// surface (PL§15.6).
    pub desktop: DesktopOut,
}

/// At most this many toolbar actions per plugin (PL§15.4).
pub const MAX_TOOLBAR_ACTIONS: usize = 2;
/// At most this many actions on one desktop notification (PL§15.4).
pub const MAX_NOTICE_ACTIONS: usize = 2;

/// What `InitOut.desktop` carries (PL§15.4). The app draws each one natively
/// from this data; a plugin never supplies UI code.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct DesktopOut {
    /// One inspector tab, filled by `cox_render` with `Slot::DesktopInspector`.
    pub inspector: Option<TabDecl>,
    /// Toolbar items, at most [`MAX_TOOLBAR_ACTIONS`].
    pub toolbar: Vec<ActionDecl>,
    /// Command-palette actions.
    pub palette: Vec<ActionDecl>,
}

/// The inspector tab a plugin adds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TabDecl {
    /// The tab's title.
    pub title: String,
    /// An SF Symbol name; see [`is_symbol_name`].
    pub symbol: String,
}

/// A toolbar item or palette action. A click goes through `cox_command`, so
/// it cannot submit anything a command could not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ActionDecl {
    /// One of the plugin's `InitOut.commands` names.
    pub command: String,
    /// The label, or the tooltip for a toolbar icon.
    pub title: String,
    /// An SF Symbol name; see [`is_symbol_name`].
    pub symbol: String,
}

/// Whether `name` is a symbol name the host passes on: `[a-z0-9.]{1,64}`.
/// Checked in the host because the name reaches AppKit, which would take any
/// string; a name that is valid but unknown draws a fallback glyph there.
pub fn is_symbol_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.')
}

/// `cox_desktop_notify` input (`cox:desktop/v1`, PL§15.4): a notification
/// whose buttons run the plugin's own commands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DesktopNotice {
    /// Severity, capped at `warn` like every plugin notice.
    pub level: NoticeLevel,
    /// The headline.
    pub title: String,
    /// The detail.
    #[serde(default)]
    pub body: String,
    /// Buttons, at most [`MAX_NOTICE_ACTIONS`].
    #[serde(default)]
    pub actions: Vec<NoticeAction>,
}

/// A button on a [`DesktopNotice`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NoticeAction {
    /// The button's label.
    pub label: String,
    /// One of the plugin's `InitOut.commands` names.
    pub command: String,
}

/// A slash command a plugin offers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CommandDecl {
    /// Name after `/<id>:`.
    pub name: String,
    /// One line for the palette.
    #[serde(default)]
    pub description: String,
}

/// A key under the plugin leader; pressing it calls `cox_key` with `name`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct KeyDecl {
    /// The key as `keybindings.toml` spells it.
    pub key: String,
    /// The name passed in `CommandIn`.
    pub name: String,
    /// One line for the help overlay.
    #[serde(default)]
    pub description: String,
}

/// A TUI slot (PL§8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Slot {
    /// Left segment of the status row.
    #[serde(rename = "status.left")]
    StatusLeft,
    /// Right segment of the status row.
    #[serde(rename = "status.right")]
    StatusRight,
    /// Rows above the composer.
    #[serde(rename = "panel")]
    Panel,
    /// Full-screen overlay.
    #[serde(rename = "overlay")]
    Overlay,
    /// The desktop inspector tab (PL§15.4); asked for only after the plugin
    /// declared `InitOut.desktop.inspector`, so an older guest never sees it.
    #[serde(rename = "desktop.inspector")]
    DesktopInspector,
}

/// `cox_on_event` input (PL§5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EventBatch {
    /// Sequence number of the first event in `events`.
    pub first_seq: u64,
    /// Events dropped from the ring since the last batch.
    pub dropped: u64,
    /// Scrubbed events, in rollout order.
    #[schemars(extend("items" = { "x-cox-protocol": "Event" }))]
    pub events: Vec<Value>,
}

/// `cox_on_event` output.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Effects {
    /// Re-render this plugin's slots.
    pub redraw: bool,
    /// Notices to show.
    pub notices: Vec<PluginNotice>,
}

/// A notice from a plugin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PluginNotice {
    /// Severity, capped at `warn`.
    pub level: NoticeLevel,
    /// Shown sanitized.
    pub text: String,
}

/// The notice levels a plugin may use. Not `cox_protocol::Level`: that one
/// has `budget` and `security`, which a plugin must never raise (PL§4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NoticeLevel {
    /// Informational.
    Info,
    /// Something degraded.
    Warn,
}

/// `cox_hook` input (PL§6); the output is a `HookOutcome`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HookCall {
    /// Which hook fired.
    #[schemars(extend("x-cox-protocol" = "HookEvent"))]
    pub event: Value,
    /// The hook's JSON input, as shell hooks get it on stdin.
    pub payload: Value,
}

/// `cox_tool_subject` / `cox_tool_risk` / `cox_tool_call` input: the `Tool`
/// trait serialised. Outputs are a string, a `Risk` and a `ToolOutput`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolCallIn {
    /// The tool's name inside the plugin, without `wasm__<id>__`.
    pub name: String,
    /// The model's input.
    pub input: Value,
}

/// `cox_command` / `cox_key` input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CommandIn {
    /// The command or key name the plugin declared.
    pub name: String,
    /// Text after the command; empty for a key.
    #[serde(default)]
    pub args: String,
}

/// `cox_command` / `cox_key` output. Closed on purpose: there is no
/// `Submission` variant, so a plugin cannot switch the permission mode,
/// approve a call or switch the model up (PL§4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandOut {
    /// Submitted as a `UserTurn`, like a markdown command.
    Prompt {
        /// The prompt text.
        text: String,
    },
    /// Compact now.
    Compact {
        /// What the summary should keep.
        #[serde(default)]
        focus: Option<String>,
    },
    /// Show or hide the plugin's panel.
    TogglePanel,
    /// Open the plugin's overlay.
    OpenOverlay,
    /// Select the plugin's inspector tab: the desktop counterpart of
    /// `OpenOverlay`, dropped with a notice on the terminal.
    OpenInspector,
    /// Show a notice.
    Notice(PluginNotice),
    /// Do nothing.
    Nothing,
}

/// `cox_render` input; the output is a `Widget`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RenderIn {
    /// The slot to fill.
    pub slot: Slot,
    /// Columns available.
    pub width: u16,
    /// Rows available.
    pub height: u16,
}

/// `cox_render_item` input; the output is an optional `Widget`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RenderItemIn {
    /// The renderer target that matched.
    pub target: String,
    /// The call, for a `tool:<name>` target.
    #[serde(default)]
    #[schemars(extend("x-cox-protocol" = "ToolCall"))]
    pub call: Option<Value>,
    /// The tool's output, or the assistant item for `item:assistant_message`.
    pub result: Value,
    /// Columns available.
    pub width: u16,
}

/// `cox_provider_stream` input (PL§7a); the output is `Vec<ProviderEvent>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderCall {
    /// The provider-neutral request.
    #[schemars(extend("x-cox-protocol" = "Request"))]
    pub request: Value,
    /// The model id to send.
    pub model: String,
}

/// `cox_model_call` input (PL§7d); the output is `Vec<ProviderEvent>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModelCall {
    /// The tier to route at; the host clamps it to the grant, never `think`.
    pub tier: ModelTier,
    /// The request; the host sets its tier, model and job.
    #[schemars(extend("x-cox-protocol" = "Request"))]
    pub request: Value,
}

/// `cox_http` input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HttpReq {
    /// HTTP method.
    pub method: String,
    /// Full URL; its host must be allowed in this context.
    pub url: String,
    /// Request headers.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// UTF-8 body.
    #[serde(default)]
    pub body: Option<String>,
}

/// `cox_http` output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HttpResp {
    /// HTTP status.
    pub status: u16,
    /// Response headers; repeated ones are joined with `, `.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// UTF-8 body, capped at `max_http_response_bytes`.
    #[serde(default)]
    pub body: String,
}

/// A decision point (PL§4 "Decision points").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DecidePoint {
    /// Tier for a user turn; only down.
    Route,
    /// Severity of a tool call; only up.
    Risk,
    /// A caution note on the approval modal; never "looks safe".
    ApproveHint,
    /// Compact now; only earlier.
    Compact,
    /// Reorder or filter candidates; never add.
    Rank,
    /// Salience of a memory item.
    Salience,
}

/// `cox_decide` input: the core offers the options and keeps the decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Question {
    /// Which point asks.
    pub point: DecidePoint,
    /// What the answer is about, redacted.
    pub state: Value,
    /// The choices the core offers (tiers for `route`, candidates for
    /// `rank`); empty for a score or yes/no point.
    #[serde(default)]
    pub options: Vec<String>,
}

/// `cox_decide` output. The core applies the point's monotone rule and
/// uses its local default when confidence is below `min_confidence`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Advice {
    /// The answer.
    pub answer: Answer,
    /// In `[0, 1]`; absent when the model gives none (a yes/no, J11).
    #[serde(default)]
    pub confidence: Option<f64>,
    /// A caution note, for `approve_hint`.
    #[serde(default)]
    pub note: Option<String>,
}

/// The three question kinds a decision model answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Answer {
    /// Indices into `Question.options`, best first.
    Choice {
        /// The ranking.
        order: Vec<u32>,
    },
    /// A score on the point's scale.
    Score {
        /// The score.
        value: f64,
    },
    /// A yes/no probability.
    Noul {
        /// P(yes) in `[0, 1]`.
        p_yes: f64,
    },
    /// One score per item of a batched question (`salience`, T33.21.1):
    /// `Question` has no `items` field yet (that is T33.40.1's batched-ABI
    /// card), so the items travel in `Question.state["items"]` and the
    /// answer carries one value per item, aligned by index.
    Scores {
        /// One score per item, same length and order as the items sent.
        values: Vec<f64>,
    },
}

/// Why a host function refused or failed.
#[derive(Debug, Clone, PartialEq, Error, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AbiError {
    /// Not allowed from the calling export (PL§4, deadlock and render caps).
    #[error("not allowed in this context")]
    NotInThisContext,
    /// A surface-only host function called on another surface. Checked
    /// before the grant: "not granted" would mislead for something that can
    /// never work here (PL§15.6).
    #[error("not available on the {} surface", .surface.name())]
    NotOnThisSurface {
        /// The surface the session runs on.
        surface: Surface,
    },
    /// The capability is not granted.
    #[error("capability {capability} is not granted")]
    NotGranted {
        /// The missing capability.
        capability: String,
    },
    /// The permission engine, a hook or the user said no.
    #[error("denied: {reason}")]
    Denied {
        /// Why.
        reason: String,
    },
    /// The budget gate refused a model call.
    #[error("budget exhausted")]
    Budget,
    /// The kv quota or a size cap was hit.
    #[error("over the limit of {limit} bytes")]
    TooLarge {
        /// The cap.
        limit: u64,
    },
    /// The call's deadline passed.
    #[error("timed out")]
    Timeout,
    /// Worth retrying (a 429/529 or a dropped connection).
    #[error("transient failure: {message}")]
    Transient {
        /// Detail.
        message: String,
    },
    /// Malformed input or any other failure.
    #[error("{message}")]
    Failed {
        /// Detail.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use serde::de::DeserializeOwned;
    use serde_json::json;

    use super::*;

    fn round_trip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(v: &T) {
        let text = serde_json::to_string(v).expect("serializes");
        let back: T = serde_json::from_str(&text).expect("deserializes");
        assert_eq!(&back, v, "{text}");
    }

    #[test]
    fn command_out_has_no_submission_variant() {
        let notice = PluginNotice {
            level: NoticeLevel::Warn,
            text: "hi".into(),
        };
        let all = [
            CommandOut::Prompt { text: "go".into() },
            CommandOut::Compact { focus: None },
            CommandOut::Compact {
                focus: Some("tests".into()),
            },
            CommandOut::TogglePanel,
            CommandOut::OpenOverlay,
            CommandOut::OpenInspector,
            CommandOut::Notice(notice),
            CommandOut::Nothing,
        ];
        let mut kinds = Vec::new();
        for out in &all {
            round_trip(out);
            let v = serde_json::to_value(out).expect("serializes");
            kinds.push(v["kind"].as_str().expect("tagged").to_owned());
        }
        kinds.dedup();
        assert_eq!(
            kinds,
            [
                "prompt",
                "compact",
                "toggle_panel",
                "open_overlay",
                "open_inspector",
                "notice",
                "nothing"
            ]
        );
        // Submission variants (and their serde tags) a plugin must not reach.
        for kind in [
            "user_turn",
            "submission",
            "set_permission_mode",
            "approve",
            "switch_model",
        ] {
            let parsed =
                serde_json::from_value::<CommandOut>(json!({ "kind": kind, "mode": "bypass" }));
            assert!(parsed.is_err(), "{kind} must not parse");
        }
        // A plugin cannot raise a notice past `warn`.
        for level in ["budget", "security"] {
            let v = json!({ "kind": "notice", "level": level, "text": "x" });
            assert!(serde_json::from_value::<CommandOut>(v).is_err(), "{level}");
        }
    }

    #[test]
    fn unknown_fields_are_ignored_both_ways() {
        // Host → guest: a newer host adds fields an older guest ignores.
        let init: InitIn = serde_json::from_value(json!({
            "api": 1, "plugin_id": "jev", "config": {}, "granted": {"kv": true},
            "session": { "id": "s1", "cwd": "/w", "surface": "tui" },
            "added_in_1_1": true,
        }))
        .expect("host → guest");
        assert_eq!(init.session.cwd, "/w");
        let q: Question = serde_json::from_value(json!({
            "point": "risk", "state": "rm -rf", "budget_ms": 200,
        }))
        .expect("question");
        assert!(q.options.is_empty());

        // Guest → host: a newer guest adds fields an older host ignores, and
        // may omit everything optional.
        let out: InitOut = serde_json::from_value(json!({
            "commands": [{ "name": "glance", "icon": "*" }],
            "widgets_v2": [],
        }))
        .expect("guest → host");
        assert_eq!(out.commands[0].name, "glance");
        let fx: Effects =
            serde_json::from_value(json!({ "redraw": true, "sound": "ding" })).expect("effects");
        assert!(fx.redraw);
        let advice: Advice = serde_json::from_value(json!({
            "answer": { "kind": "noul", "p_yes": 0.9, "probabilities": [0.1, 0.9] },
            "latency_ms": 12,
        }))
        .expect("advice");
        assert_eq!(advice.answer, Answer::Noul { p_yes: 0.9 });
        let cmd: CommandOut =
            serde_json::from_value(json!({ "kind": "prompt", "text": "go", "extra": 1 }))
                .expect("command out");
        assert_eq!(cmd, CommandOut::Prompt { text: "go".into() });
        let err: AbiError = serde_json::from_value(json!({ "kind": "budget", "remaining_usd": 0 }))
            .expect("abi error");
        assert_eq!(err, AbiError::Budget);

        // The surface parts follow the same rule (PL§15.7).
        let init: InitIn = serde_json::from_value(json!({
            "api": 1, "plugin_id": "jev", "config": {}, "granted": {},
            "session": { "id": "s1", "cwd": "/w" }, "surface": "desktop", "added": 1,
        }))
        .expect("host → guest");
        assert_eq!(init.surface, Some(Surface::Desktop));
        let out: InitOut = serde_json::from_value(json!({
            "desktop": {
                "inspector": { "title": "Glance", "symbol": "eye", "badge": 3 },
                "toolbar": [{ "command": "go", "title": "Go", "symbol": "play", "hint": "x" }],
                "sidebar": [],
            },
        }))
        .expect("guest → host");
        assert_eq!(out.desktop.inspector.expect("tab").title, "Glance");
        assert_eq!(out.desktop.toolbar[0].command, "go");
        let notice: DesktopNotice = serde_json::from_value(json!({
            "level": "warn", "title": "t", "sound": "ding",
            "actions": [{ "label": "Open", "command": "go", "style": "primary" }],
        }))
        .expect("desktop notice");
        assert_eq!(notice.actions.len(), 1);
        assert!(notice.body.is_empty());
        let err: AbiError = serde_json::from_value(
            json!({ "kind": "not_on_this_surface", "surface": "acp", "x": 1 }),
        )
        .expect("abi error");
        assert_eq!(
            err,
            AbiError::NotOnThisSurface {
                surface: Surface::Acp
            }
        );
    }

    #[test]
    fn init_in_without_surface_reads_as_none() {
        let init: InitIn = serde_json::from_value(json!({
            "api": 1, "plugin_id": "jev", "config": {}, "granted": {},
            "session": { "id": "s1", "cwd": "/w" },
        }))
        .expect("an older host's init");
        assert_eq!(init.surface, None);
    }

    #[test]
    fn surface_round_trips_and_uses_lowercase_names() {
        for (surface, name) in [
            (Surface::Terminal, "terminal"),
            (Surface::Desktop, "desktop"),
            (Surface::Headless, "headless"),
            (Surface::Acp, "acp"),
        ] {
            round_trip(&surface);
            assert_eq!(serde_json::to_value(surface).expect("serializes"), name);
            assert_eq!(surface.name(), name);
        }
        assert!(serde_json::from_value::<Surface>(json!("tui")).is_err());
    }

    #[test]
    fn desktop_payloads_round_trip() {
        let action = ActionDecl {
            command: "go".into(),
            title: "Go".into(),
            symbol: "play.fill".into(),
        };
        round_trip(&InitOut {
            desktop: DesktopOut {
                inspector: Some(TabDecl {
                    title: "Glance".into(),
                    symbol: "eye".into(),
                }),
                toolbar: vec![action.clone()],
                palette: vec![action],
            },
            ..InitOut::default()
        });
        round_trip(&DesktopNotice {
            level: NoticeLevel::Info,
            title: "Done".into(),
            body: "all green".into(),
            actions: vec![NoticeAction {
                label: "Open".into(),
                command: "go".into(),
            }],
        });
        round_trip(&AbiError::NotOnThisSurface {
            surface: Surface::Terminal,
        });
        round_trip(&RenderIn {
            slot: Slot::DesktopInspector,
            width: 40,
            height: 12,
        });
    }

    #[test]
    fn inspector_slot_is_spelled_desktop_dot_inspector() {
        let v = serde_json::to_value(Slot::DesktopInspector).expect("serializes");
        assert_eq!(v, "desktop.inspector");
        assert_eq!(
            AbiError::NotOnThisSurface {
                surface: Surface::Terminal
            }
            .to_string(),
            "not available on the terminal surface"
        );
    }

    #[test]
    fn symbol_names_are_lowercase_ascii_dotted_and_bounded() {
        assert!(is_symbol_name("puzzlepiece.extension"));
        assert!(is_symbol_name(&"a".repeat(64)));
        for bad in ["", "Eye", "a b", "a/b", "é", &"a".repeat(65)] {
            assert!(!is_symbol_name(bad), "{bad}");
        }
    }
}
