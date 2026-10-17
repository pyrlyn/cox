//! `cox-plugin-sdk`: write a cox plugin in Rust. Typed wrappers over every
//! guest export and host function of ABI v1 (PL§4, `cox:host/v1`, and
//! `cox:desktop/v1` from PL§15), on top of
//! extism-pdk's memory and I/O, with the payload types from `cox-plugin-api`.
//! A crate of its own in the guest workspace `plugins/` because it builds for
//! `wasm32-unknown-unknown` and must never build for the host in the main
//! workspace's `cargo nextest run --workspace` (PL§9).
//!
//! The wire, which a host or a guest in another language must match
//! (`docs/plugins.md`):
//! - an export reads one JSON value (`null` when the host sends no input) and
//!   writes one JSON value; a handler error becomes the extism call error and
//!   return code 1;
//! - a host function takes one memory block holding one JSON value (its only
//!   argument, an object keyed by its argument names when it has several, or
//!   `null` when it has none) and returns a block holding `{"Ok": value}` or
//!   `{"Err": AbiError}`, so a refusal reaches the plugin as a value instead
//!   of a trap that would end the call.
//!
//! ```ignore
//! use cox_plugin_sdk::*;
//! fn init(_: InitIn) -> Result<InitOut, SdkError> { Ok(InitOut::default()) }
//! fn command(c: CommandIn) -> Result<CommandOut, SdkError> { Ok(CommandOut::Nothing) }
//! cox_plugin_sdk::register!(init => init, command => command);
//! ```
//!
//! (`ignore` because the example defines `#[no_mangle]` exports; the test
//! module registers every key instead.)

pub use cox_plugin_api::*;
pub use serde_json::Value;

use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

/// Why an SDK call failed.
#[derive(Debug, Error)]
pub enum SdkError {
    /// The host refused or failed the call (PL§4): not granted, not allowed
    /// in this export, denied, over budget or a cap, timed out.
    #[error(transparent)]
    Host(#[from] AbiError),
    /// A payload did not encode or decode, or the host returned no block.
    #[error("abi wire: {0}")]
    Wire(String),
}

impl From<serde_json::Error> for SdkError {
    fn from(e: serde_json::Error) -> Self {
        Self::Wire(e.to_string())
    }
}

/// What a host function wrapper returns.
pub type SdkResult<T> = Result<T, SdkError>;

/// `cox_kv_put`'s two arguments on the wire.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Serialize)]
struct KvPut<'a> {
    key: &'a str,
    value: &'a Value,
}

/// The `cox:host/v1` imports, raw. Every one has the same shape, so one
/// generic caller (`call`) encodes the argument, decodes the `Ok`/`Err`
/// envelope and frees both blocks; `#[host_fn]` would type each import but
/// leave the envelope and the reply block to every wrapper.
#[cfg(target_arch = "wasm32")]
mod ffi {
    #[link(wasm_import_module = "cox:host/v1")]
    unsafe extern "C" {
        pub fn cox_log(args: u64) -> u64;
        pub fn cox_notify(args: u64) -> u64;
        pub fn cox_kv_get(args: u64) -> u64;
        pub fn cox_kv_put(args: u64) -> u64;
        pub fn cox_kv_delete(args: u64) -> u64;
        pub fn cox_context(args: u64) -> u64;
        pub fn cox_invoke_tool(args: u64) -> u64;
        pub fn cox_model_call(args: u64) -> u64;
        pub fn cox_http(args: u64) -> u64;
        pub fn cox_output(args: u64) -> u64;
        pub fn cox_cancelled(args: u64) -> u64;
        pub fn cox_redraw(args: u64) -> u64;
    }

    // Its own import module (PL§15.4): a plugin that never calls it imports
    // nothing from it, so it still loads in a cox that predates the section.
    #[link(wasm_import_module = "cox:desktop/v1")]
    unsafe extern "C" {
        pub fn cox_desktop_notify(args: u64) -> u64;
    }
}

/// Decodes a host function's reply block: `{"Ok": T}` or `{"Err": AbiError}`.
#[cfg(any(target_arch = "wasm32", test))]
fn decode_reply<T: DeserializeOwned>(bytes: &[u8]) -> SdkResult<T> {
    Ok(serde_json::from_slice::<Result<T, AbiError>>(bytes)??)
}

#[cfg(target_arch = "wasm32")]
fn call<T: DeserializeOwned>(
    import: unsafe extern "C" fn(u64) -> u64,
    args: &impl Serialize,
) -> SdkResult<T> {
    use extism_pdk::Memory;
    let arg =
        Memory::from_bytes(serde_json::to_vec(args)?).map_err(|e| SdkError::Wire(e.to_string()))?;
    // SAFETY: the import is a `cox:host/v1` function that reads the block at
    // this offset and returns the offset of a block it allocated.
    let reply = unsafe { import(arg.offset()) };
    arg.free();
    let reply = Memory::find(reply).ok_or_else(|| SdkError::Wire("no reply block".into()))?;
    let bytes = reply.to_vec();
    reply.free();
    decode_reply(&bytes)
}

/// Defines one wrapper per host function; the expression after `=` is the
/// JSON value the wire rule above sends.
macro_rules! host_fns {
    ($( $(#[$doc:meta])* $name:ident => $import:ident ($($arg:ident: $ty:ty),*) -> $out:ty = $wire:expr; )*) => {$(
        $(#[$doc])*
        pub fn $name($($arg: $ty),*) -> SdkResult<$out> {
            #[cfg(target_arch = "wasm32")]
            { call(ffi::$import, &$wire) }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // A host build exists only for unit tests of the pure parts.
                let _ = ($($arg,)*);
                Err(SdkError::Wire(concat!(stringify!($import), " needs wasm32").into()))
            }
        }
    )*};
}

host_fns! {
    /// `cox_log`: a line in cox's log (`tracing`), rate-limited.
    log => cox_log(level: NoticeLevel, text: &str) -> () = PluginNotice { level, text: text.into() };
    /// `cox_notify`: a notice in the transcript, shown sanitized.
    notify => cox_notify(level: NoticeLevel, text: &str) -> () = PluginNotice { level, text: text.into() };
    /// `cox_kv_get` (capability `kv`): the stored value, if any.
    kv_get => cox_kv_get(key: &str) -> Option<Value> = key;
    /// `cox_kv_put` (capability `kv`): stores a value within the quota.
    kv_put => cox_kv_put(key: &str, value: &Value) -> () = KvPut { key, value };
    /// `cox_kv_delete` (capability `kv`).
    kv_delete => cox_kv_delete(key: &str) -> () = key;
    /// `cox_context` (capability `context`): the host's event-folded snapshot.
    context => cox_context() -> Value = ();
    /// `cox_invoke_tool` (capability `invoke` lists `name`): runs the tool
    /// through the permission engine like a model call; returns a `ToolOutput`.
    invoke_tool => cox_invoke_tool(name: &str, input: Value) -> Value = ToolCallIn { name: name.into(), input };
    /// `cox_model_call` (capability `model`): returns `Vec<ProviderEvent>`.
    model_call => cox_model_call(call: &ModelCall) -> Vec<Value> = call;
    /// `cox_http` (capability `net`, or a provider host inside
    /// `cox_provider_stream`).
    http => cox_http(request: &HttpReq) -> HttpResp = request;
    /// `cox_output`: one progress line, inside `cox_tool_call` only.
    output => cox_output(line: &str) -> () = line;
    /// `cox_cancelled`: whether the running tool call was cancelled.
    cancelled => cox_cancelled() -> bool = ();
    /// `cox_redraw` (any `ui` capability): marks the plugin's slots dirty.
    redraw => cox_redraw() -> () = ();
    /// `cox_desktop_notify` (capability `desktop.notify`, `cox:desktop/v1`,
    /// PL§15.4): a notification whose buttons run the plugin's own commands.
    /// Off the desktop it is `Err(NotOnThisSurface)`, which a plugin treats as
    /// "use `notify`", not as a failure.
    desktop_notify => cox_desktop_notify(notice: &DesktopNotice) -> () = notice;
}

/// The surface `init` says the session runs on. A cox older than PL§15 omits
/// it and only ever loaded plugins into a terminal session, so absence reads
/// as `terminal`.
pub fn surface_of(init: &InitIn) -> Surface {
    init.surface.unwrap_or(Surface::Terminal)
}

/// Runs one export on its raw input and returns its raw output, or the
/// message the host sees as the call error.
fn run_export<I, O, E>(
    input: &[u8],
    handler: impl FnOnce(I) -> Result<O, E>,
) -> Result<Vec<u8>, String>
where
    I: DeserializeOwned,
    O: Serialize,
    E: std::fmt::Display,
{
    // `cox_shutdown` takes `()`, which a host may send as no bytes at all.
    let input = if input.is_empty() {
        b"null".as_slice()
    } else {
        input
    };
    let input = serde_json::from_slice(input).map_err(|e| format!("bad input: {e}"))?;
    let output = handler(input).map_err(|e| e.to_string())?;
    serde_json::to_vec(&output).map_err(|e| format!("bad output: {e}"))
}

/// The body of every export `register!` defines. Not API.
#[doc(hidden)]
pub fn __export<I, O, E>(handler: impl FnOnce(I) -> Result<O, E>) -> i32
where
    I: DeserializeOwned,
    O: Serialize,
    E: std::fmt::Display,
{
    let result = run_export(&extism_pdk::input_bytes(), handler).and_then(|bytes| {
        let out = extism_pdk::Memory::from_bytes(bytes).map_err(|e| e.to_string())?;
        out.set_output();
        Ok(())
    });
    match result {
        Ok(()) => 0,
        Err(message) => {
            if let Ok(block) = extism_pdk::Memory::from_bytes(message) {
                // SAFETY: the block was just allocated in extism memory; the
                // host reads it as this call's error text.
                unsafe { extism_pdk::extism::error_set(block.offset()) };
            }
            1
        }
    }
}

/// Defines the plugin's exports. `init` is required and first; every other
/// key is optional and exported only when listed, because the host probes
/// exports with `function_exists` (PL§4). A handler is a function from the
/// export's input to `Result<output, E>` for any `E: Display`:
///
/// | key | export | input → output |
/// | --- | --- | --- |
/// | `init` | `cox_init` | `InitIn` → `InitOut` |
/// | `on_event` | `cox_on_event` | `EventBatch` → `Effects` |
/// | `hook` | `cox_hook` | `HookCall` → `Value` (`HookOutcome`) |
/// | `decide` | `cox_decide` | `Question` → `Advice` |
/// | `decide_call` | `cox_decide` | `Question` → `DecideOut` (two-phase) |
/// | `decide_resume` | `cox_decide_resume` | `DecideResume` → `Option<Advice>` |
/// | `tool_subject` | `cox_tool_subject` | `ToolCallIn` → `String` |
/// | `tool_risk` | `cox_tool_risk` | `ToolCallIn` → `Value` (`Risk`) |
/// | `tool_call` | `cox_tool_call` | `ToolCallIn` → `Value` (`ToolOutput`) |
/// | `provider_stream` | `cox_provider_stream` | `ProviderCall` → `Vec<Value>` (`ProviderEvent`s) |
/// | `command` / `key` | `cox_command` / `cox_key` | `CommandIn` → `CommandOut` |
/// | `render` | `cox_render` | `RenderIn` → `Widget` |
/// | `render_item` | `cox_render_item` | `RenderItemIn` → `Option<Widget>` |
/// | `canvas` | `cox_canvas` | `CanvasIn` → `CanvasOut` |
/// | `canvas_select` | `cox_canvas_select` | `CanvasSelect` → `Option<CanvasAttach>` |
/// | `shutdown` | `cox_shutdown` | `()` → `()` |
#[macro_export]
macro_rules! register {
    (init => $init:path $(, $key:ident => $handler:path)* $(,)?) => {
        $crate::__export_fn!(init, $init);
        $( $crate::__export_fn!($key, $handler); )*
    };
}

/// Maps a `register!` key to its export name and types. Not API.
#[doc(hidden)]
#[macro_export]
macro_rules! __export_fn {
    (init, $f:path) => { $crate::__export_fn!(@ cox_init, $crate::InitIn, $crate::InitOut, $f); };
    (on_event, $f:path) => { $crate::__export_fn!(@ cox_on_event, $crate::EventBatch, $crate::Effects, $f); };
    (hook, $f:path) => { $crate::__export_fn!(@ cox_hook, $crate::HookCall, $crate::Value, $f); };
    (decide, $f:path) => { $crate::__export_fn!(@ cox_decide, $crate::Question, $crate::Advice, $f); };
    // The two-phase form (T33.40.1): `cox_decide` may hand the host a model
    // call, whose events come back through `cox_decide_resume`. A separate
    // key because both forms export `cox_decide` with different outputs.
    (decide_call, $f:path) => { $crate::__export_fn!(@ cox_decide, $crate::Question, $crate::DecideOut, $f); };
    (decide_resume, $f:path) => {
        $crate::__export_fn!(@ cox_decide_resume, $crate::DecideResume, Option<$crate::Advice>, $f);
    };
    (tool_subject, $f:path) => { $crate::__export_fn!(@ cox_tool_subject, $crate::ToolCallIn, String, $f); };
    (tool_risk, $f:path) => { $crate::__export_fn!(@ cox_tool_risk, $crate::ToolCallIn, $crate::Value, $f); };
    (tool_call, $f:path) => { $crate::__export_fn!(@ cox_tool_call, $crate::ToolCallIn, $crate::Value, $f); };
    (provider_stream, $f:path) => {
        $crate::__export_fn!(@ cox_provider_stream, $crate::ProviderCall, Vec<$crate::Value>, $f);
    };
    (command, $f:path) => { $crate::__export_fn!(@ cox_command, $crate::CommandIn, $crate::CommandOut, $f); };
    (key, $f:path) => { $crate::__export_fn!(@ cox_key, $crate::CommandIn, $crate::CommandOut, $f); };
    (render, $f:path) => { $crate::__export_fn!(@ cox_render, $crate::RenderIn, $crate::Widget, $f); };
    (render_item, $f:path) => {
        $crate::__export_fn!(@ cox_render_item, $crate::RenderItemIn, Option<$crate::Widget>, $f);
    };
    (canvas, $f:path) => { $crate::__export_fn!(@ cox_canvas, $crate::CanvasIn, $crate::CanvasOut, $f); };
    (canvas_select, $f:path) => {
        $crate::__export_fn!(@ cox_canvas_select, $crate::CanvasSelect, Option<$crate::CanvasAttach>, $f);
    };
    (shutdown, $f:path) => { $crate::__export_fn!(@ cox_shutdown, (), (), $f); };
    (@ $name:ident, $in:ty, $out:ty, $f:path) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name() -> i32 {
            $crate::__export::<$in, $out, _>($f)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_refusal_decodes_as_abi_error() {
        let bytes = br#"{"Err":{"kind":"not_granted","capability":"kv"}}"#;
        let err = decode_reply::<Option<Value>>(bytes).expect_err("a refusal");
        assert!(matches!(
            err,
            SdkError::Host(AbiError::NotGranted { ref capability }) if capability == "kv"
        ));
    }

    #[test]
    fn host_success_decodes_ok_payload() {
        assert!(decode_reply::<bool>(br#"{"Ok":true}"#).expect("ok"));
        decode_reply::<()>(br#"{"Ok":null}"#).expect("unit ok");
    }

    #[test]
    fn malformed_reply_is_a_wire_error() {
        assert!(matches!(
            decode_reply::<bool>(b"true"),
            Err(SdkError::Wire(_))
        ));
    }

    #[test]
    fn kv_put_sends_an_object_keyed_by_argument_names() {
        let value = Value::from(3);
        let wire = serde_json::to_value(KvPut {
            key: "turns",
            value: &value,
        })
        .expect("serializes");
        assert_eq!(wire, serde_json::json!({ "key": "turns", "value": 3 }));
    }

    #[test]
    fn surface_is_terminal_when_the_host_does_not_say() {
        let mut init: InitIn = serde_json::from_value(serde_json::json!({
            "api": 1, "plugin_id": "p", "config": null,
            "session": { "id": "s", "cwd": "/w" }, "granted": {},
        }))
        .expect("an InitIn from a pre-PL§15 host");
        assert_eq!(surface_of(&init), Surface::Terminal);
        init.surface = Some(Surface::Desktop);
        assert_eq!(surface_of(&init), Surface::Desktop);
    }

    #[test]
    fn surface_refusal_reaches_the_plugin_as_a_value() {
        let bytes = br#"{"Err":{"kind":"not_on_this_surface","surface":"terminal"}}"#;
        let err = decode_reply::<()>(bytes).expect_err("a refusal");
        assert!(matches!(
            err,
            SdkError::Host(AbiError::NotOnThisSurface {
                surface: Surface::Terminal
            })
        ));
    }

    #[test]
    fn empty_input_reaches_a_unit_export() {
        let out = run_export(b"", |(): ()| Ok::<_, SdkError>(())).expect("runs");
        assert_eq!(out, b"null");
    }

    #[test]
    fn export_round_trips_typed_payloads() {
        let input = br#"{"name":"reset","args":""}"#;
        let out = run_export(input, |c: CommandIn| {
            Ok::<_, SdkError>(CommandOut::Prompt { text: c.name })
        })
        .expect("runs");
        let back: CommandOut = serde_json::from_slice(&out).expect("decodes");
        assert_eq!(
            back,
            CommandOut::Prompt {
                text: "reset".into()
            }
        );
    }

    #[test]
    fn handler_and_input_errors_become_the_call_error() {
        let failed = run_export(b"{}", |_: Value| Err::<(), _>("boom"));
        assert_eq!(failed, Err("boom".into()));
        let bad = run_export(b"not json", |v: Value| Ok::<_, SdkError>(v));
        assert!(bad.is_err_and(|e| e.starts_with("bad input")));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn host_fns_refuse_off_wasm() {
        assert!(matches!(redraw(), Err(SdkError::Wire(_))));
        let notice = DesktopNotice {
            level: NoticeLevel::Info,
            title: "t".into(),
            body: String::new(),
            actions: Vec::new(),
        };
        assert!(matches!(desktop_notify(&notice), Err(SdkError::Wire(_))));
    }

    /// Every `register!` key, so a key whose types drift from `cox-plugin-api`
    /// fails to compile here rather than in a plugin author's build.
    mod every_key_registers {
        use crate::*;

        type R<T> = Result<T, SdkError>;
        fn init(_: InitIn) -> R<InitOut> {
            Ok(InitOut::default())
        }
        fn on_event(_: EventBatch) -> R<Effects> {
            Ok(Effects::default())
        }
        fn value(_: ToolCallIn) -> R<Value> {
            Ok(Value::Null)
        }
        fn hook(_: HookCall) -> R<Value> {
            Ok(Value::Null)
        }
        fn decide(q: Question) -> R<Advice> {
            let _ = q;
            Err(SdkError::Wire("abstain".into()))
        }
        fn decide_resume(_: DecideResume) -> R<Option<Advice>> {
            Ok(None)
        }
        fn subject(c: ToolCallIn) -> R<String> {
            Ok(c.name)
        }
        fn provider(_: ProviderCall) -> R<Vec<Value>> {
            Ok(Vec::new())
        }
        fn command(_: CommandIn) -> R<CommandOut> {
            Ok(CommandOut::Nothing)
        }
        fn render(_: RenderIn) -> R<Widget> {
            Ok(Widget::Text(Vec::new()))
        }
        fn render_item(_: RenderItemIn) -> R<Option<Widget>> {
            Ok(None)
        }
        fn canvas(_: CanvasIn) -> R<CanvasOut> {
            Ok(CanvasOut::default())
        }
        fn canvas_select(_: CanvasSelect) -> R<Option<CanvasAttach>> {
            Ok(None)
        }
        fn shutdown(_: ()) -> R<()> {
            Ok(())
        }

        crate::register!(
            init => init,
            on_event => on_event,
            hook => hook,
            decide => decide,
            decide_resume => decide_resume,
            tool_subject => subject,
            tool_risk => value,
            tool_call => value,
            provider_stream => provider,
            command => command,
            key => command,
            render => render,
            render_item => render_item,
            canvas => canvas,
            canvas_select => canvas_select,
            shutdown => shutdown,
        );
    }
}
