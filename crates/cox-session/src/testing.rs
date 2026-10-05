// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Fixtures shared by this crate's tests and `crates/cox`'s (the
//! `test-util` feature): a scripted session, a user turn, a granted plugin
//! installed the way `cox plugin install` leaves it, and a provider that
//! records every request. One copy, so the two crates' plugin tests build
//! the same packages. Panics are the point here: a fixture that cannot be
//! built fails the test that asked for it.

use std::path::Path;
use std::sync::Arc;

use cox_core::Session;
use cox_protocol::Config;
#[cfg(feature = "plugins")]
use cox_protocol::GrantScope;
use cox_protocol::traits::{Provider, Store as _};
use cox_protocol::types::{ProviderId, Submission};
use cox_store::Store;

/// A session over a scripted provider playing `scenario`, with no tools.
pub fn scripted_session(home: &Path, work: &Path, scenario: &str) -> (Session, Arc<Store>) {
    let store = Arc::new(Store::open(home).expect("store"));
    let provider: Arc<dyn Provider> =
        Arc::new(cox_provider::scripted::Scripted::from_toml(scenario, "").expect("scenario"));
    let session = Session::new(
        Config::default(),
        provider,
        vec![],
        store.clone(),
        store.clone(),
        work.to_path_buf(),
    )
    .expect("session");
    (session, store)
}

/// Submits one user turn and waits for it to finish.
pub async fn user_turn(session: &Session, text: &str) {
    session
        .submit(Submission::UserTurn {
            text: text.into(),
            attachments: vec![],
            confirm_think: false,
        })
        .await
        .expect("turn");
}

/// T33.44 fixture: a plugin module. `cox_init` `cox_notify`s
/// `init_note` (when not empty) and answers `init_out`; `cox_hook`
/// counts its calls, `cox_notify`s `hook_note` (when not empty) and
/// answers `continue`; `cox_on_event` answers one notice, `hooks <n>`.
#[cfg(feature = "plugins")]
pub fn plugin_wat(init_note: &str, init_out: &str, hook_note: &str) -> String {
    let note = |text: &str| format!(r#"{{"level":"info","text":"{text}"}}"#);
    let effects = r#"{"redraw":false,"notices":[{"level":"info","text":"hooks 0"}]}"#;
    let digit = 1024 + effects.find('0').expect("digit");
    let data = |at: usize, text: &str| {
        let wat = text.replace('\\', "\\\\").replace('"', "\\\"");
        format!(r#"(data (i32.const {at}) "{wat}")"#)
    };
    let notify = |at: usize, text: &str| match text {
        "" => String::new(),
        _ => format!(
            "(drop (call $notify (call $copy (i32.const {at}) (i32.const {}))))",
            note(text).len()
        ),
    };
    format!(
        r#"(module
          (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
          (import "extism:host/env" "store_u8" (func $store (param i64 i32)))
          (import "extism:host/env" "output_set" (func $output_set (param i64 i64)))
          (import "cox:host/v1" "cox_notify" (func $notify (param i64) (result i64)))
          (memory 1)
          (global $n (mut i32) (i32.const 0))
          {d0} {d1} {d2} {d3} {d4}
          (func $copy (param $p i32) (param $len i32) (result i64) (local $off i64) (local $i i32)
            (local.set $off (call $alloc (i64.extend_i32_u (local.get $len))))
            (block $done (loop $next
              (br_if $done (i32.ge_u (local.get $i) (local.get $len)))
              (call $store (i64.add (local.get $off) (i64.extend_i32_u (local.get $i)))
                (i32.load8_u (i32.add (local.get $p) (local.get $i))))
              (local.set $i (i32.add (local.get $i) (i32.const 1)))
              (br $next)))
            (local.get $off))
          (func $out (param $p i32) (param $len i32)
            (call $output_set (call $copy (local.get $p) (local.get $len))
              (i64.extend_i32_u (local.get $len))))
          (func (export "cox_init") (result i32)
            {init_notify}
            (call $out (i32.const 0) (i32.const {init_len})) (i32.const 0))
          (func (export "cox_hook") (result i32)
            (global.set $n (i32.add (global.get $n) (i32.const 1)))
            {hook_notify}
            (call $out (i32.const 768) (i32.const 19)) (i32.const 0))
          (func (export "cox_on_event") (result i32)
            (i32.store8 (i32.const {digit}) (i32.add (i32.const 48) (global.get $n)))
            (call $out (i32.const 1024) (i32.const {effects_len})) (i32.const 0)))"#,
        d0 = data(0, init_out),
        d1 = data(256, &note(init_note)),
        d2 = data(512, &note(hook_note)),
        d3 = data(768, r#"{"type":"continue"}"#),
        d4 = data(1024, effects),
        init_notify = notify(256, init_note),
        hook_notify = notify(512, hook_note),
        init_len = init_out.len(),
        effects_len = effects.len(),
    )
}

/// Installs a user plugin under `home` the way `cox plugin install`
/// leaves it, granted for its exact digest (T33.44 fixtures).
#[cfg(feature = "plugins")]
pub fn install_granted(home: &Path, id: &str, extra_toml: &str, wasm: &str) {
    let staged = home.join("plugins").join(id).join("versions/staged");
    std::fs::create_dir_all(&staged).expect("plugin dir");
    let toml = format!(
        "api = 1\nid = \"{id}\"\nversion = \"0.1.0\"\nname = \"{id}\"\nwasm = \"plugin.wasm\"\n{extra_toml}"
    );
    std::fs::write(staged.join("plugin.toml"), toml).expect("plugin.toml");
    std::fs::write(staged.join("plugin.wasm"), wasm).expect("plugin.wasm");
    let digest = cox_plugin::package_digest(&staged).expect("digest");
    std::fs::rename(&staged, staged.with_file_name(&digest[..12])).expect("stage");
    std::fs::write(home.join("plugins").join(id).join("current"), &digest[..12]).expect("current");
    let found = cox_plugin::discover::discover(home, None);
    let plugin = found.plugins.iter().find(|p| p.id == id).expect("found");
    let cox_plugin::State::Loaded { manifest, digest } = &plugin.state else {
        panic!("{id} did not load: {:?}", found.notices);
    };
    let store = Store::open(home).expect("store");
    crate::write_grant(
        &store,
        id,
        &GrantScope::User,
        digest,
        cox_plugin::grant::capability_list(manifest),
        serde_json::json!({}),
    )
    .expect("grant");
}

/// T33.12: every request a scripted session sends, for prefix checks.
pub struct Recorder {
    pub inner: cox_provider::scripted::Scripted,
    pub sent: std::sync::Mutex<Vec<cox_protocol::types::Request>>,
}

#[async_trait::async_trait]
impl Provider for Recorder {
    fn id(&self) -> ProviderId {
        self.inner.id()
    }
    fn capabilities(&self) -> cox_protocol::types::Caps {
        self.inner.capabilities()
    }
    async fn stream(
        &self,
        req: cox_protocol::types::Request,
        sink: tokio::sync::mpsc::Sender<cox_protocol::types::ProviderEvent>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<cox_protocol::types::Usage, cox_protocol::errors::ProviderError> {
        self.sent.lock().expect("sent").push(req.clone());
        self.inner.stream(req, sink, cancel).await
    }
    async fn count_tokens(
        &self,
        req: &cox_protocol::types::Request,
    ) -> Result<u32, cox_protocol::errors::ProviderError> {
        self.inner.count_tokens(req).await
    }
}
