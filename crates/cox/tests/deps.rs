// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Enforces the crate dependency-direction rules from plan.md §1.1 by
//! parsing `cargo metadata` rather than hand-maintaining a second copy of
//! the graph that could drift out of sync with the workspace `Cargo.toml`s.

use std::collections::{HashMap, HashSet};
use std::process::Command;

use serde_json::Value;

/// Maps each workspace crate name to the set of *other workspace crates* it
/// depends on (external deps like `serde` or `clap` are filtered out).
fn workspace_deps() -> HashMap<String, HashSet<String>> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .expect("cargo metadata should run");
    assert!(
        output.status.success(),
        "cargo metadata exited with {:?}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let meta: Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata output is valid json");

    let packages = meta["packages"].as_array().expect("packages array");
    let workspace_names: HashSet<String> = packages
        .iter()
        .map(|p| p["name"].as_str().expect("package name").to_string())
        .collect();

    packages
        .iter()
        .map(|pkg| {
            let name = pkg["name"].as_str().expect("package name").to_string();
            let deps = pkg["dependencies"]
                .as_array()
                .expect("dependencies array")
                .iter()
                .filter(|d| d.get("kind").and_then(|k| k.as_str()) != Some("dev"))
                .filter_map(|d| d["name"].as_str().map(str::to_string))
                .filter(|d| workspace_names.contains(d))
                .collect();
            (name, deps)
        })
        .collect()
}

/// Maps each workspace crate name to the set of *all* its declared
/// dependency names (workspace and external alike), unlike `workspace_deps`
/// which filters down to workspace crates only.
fn all_deps() -> HashMap<String, HashSet<String>> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .expect("cargo metadata should run");
    assert!(
        output.status.success(),
        "cargo metadata exited with {:?}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let meta: Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata output is valid json");

    meta["packages"]
        .as_array()
        .expect("packages array")
        .iter()
        .map(|pkg| {
            let name = pkg["name"].as_str().expect("package name").to_string();
            let deps = pkg["dependencies"]
                .as_array()
                .expect("dependencies array")
                .iter()
                .filter_map(|d| d["name"].as_str().map(str::to_string))
                .collect();
            (name, deps)
        })
        .collect()
}

/// D9/plan.md §1.7: "No other crate may depend on `diesel`" — `cox-store`
/// is the only crate allowed to contain SQL.
#[test]
fn only_store_depends_on_diesel() {
    let deps = all_deps();
    for (crate_name, crate_deps) in &deps {
        if crate_name == "cox-store" {
            continue;
        }
        for diesel_crate in ["diesel", "diesel_migrations", "libsqlite3-sys"] {
            assert!(
                !crate_deps.contains(diesel_crate),
                "{crate_name} must not depend on {diesel_crate}; only cox-store may contain SQL"
            );
        }
    }
}

/// T32.2 / `docs/design/crates.md` C2: the highlighting and markdown stack
/// lives in `cox-render` alone, so an edit to the TUI's state machine never
/// recompiles against it and no other crate grows a second renderer.
#[test]
fn only_render_depends_on_the_highlighters() {
    let deps = all_deps();
    for (crate_name, crate_deps) in &deps {
        if crate_name == "cox-render" {
            continue;
        }
        for heavy in [
            "syntect",
            "two-face",
            "pulldown-cmark",
            "terminal-colorsaurus",
        ] {
            assert!(
                !crate_deps.contains(heavy),
                "{crate_name} must not depend on {heavy}; only cox-render renders markdown and colour"
            );
        }
    }
}

/// A52/plan.md §1.1: "only `cox-plugin` depends on extism", so no other
/// crate links wasmtime. `wasmtime` itself is declared only to switch on its
/// `anyhow` feature for extism (workspace `Cargo.toml`).
#[test]
fn only_plugin_depends_on_extism() {
    let deps = all_deps();
    for (crate_name, crate_deps) in &deps {
        if crate_name == "cox-plugin" {
            continue;
        }
        for wasm_crate in ["extism", "wasmtime"] {
            assert!(
                !crate_deps.contains(wasm_crate),
                "{crate_name} must not depend on {wasm_crate}; only cox-plugin hosts WASM"
            );
        }
    }

    // cox-plugin sits beside the other implementations: the contract crates
    // and the terminal-text guard, never cox-core (plan.md §1.1).
    let plugin_allowed: HashSet<&str> = ["cox-protocol", "cox-plugin-api", "cox-sanitize"]
        .into_iter()
        .collect();
    let plugin_deps = &workspace_deps()["cox-plugin"];
    assert!(
        plugin_deps
            .iter()
            .all(|d| plugin_allowed.contains(d.as_str())),
        "cox-plugin may only depend on cox-protocol/cox-plugin-api/cox-sanitize among workspace crates, found {plugin_deps:?}"
    );
}

/// P54 (A123): whisper.cpp's C++ build and platform audio live in
/// `cox-voice` alone, and the default `cox` build never pulls it: only the
/// `voice` feature, off by default, links it.
#[test]
fn only_cox_voice_depends_on_whisper_cpal_and_rubato() {
    for (crate_name, crate_deps) in &all_deps() {
        if crate_name == "cox-voice" {
            continue;
        }
        for audio in ["whisper-rs", "cpal", "rubato"] {
            assert!(
                !crate_deps.contains(audio),
                "{crate_name} must not depend on {audio}; only cox-voice does"
            );
        }
    }
    let default = tree("cox", &[]);
    for banned in ["cox-voice", "whisper-rs", "whisper-rs-sys"] {
        assert!(
            !default.contains(banned),
            "the default cox build must not pull {banned}"
        );
    }
}

/// Names of every package in `package`'s normal dependency tree under `features`.
fn tree(package: &str, features: &[&str]) -> HashSet<String> {
    let output = Command::new("cargo")
        .args([
            "tree",
            "-p",
            package,
            "-e",
            "normal",
            "--offline",
            "--prefix",
            "none",
        ])
        .args(["--format", "{p}"])
        .args(features)
        .output()
        .expect("cargo tree should run");
    assert!(
        output.status.success(),
        "cargo tree exited with {:?}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|l| l.split_whitespace().next().map(str::to_string))
        .collect()
}

/// PL§12 falsifier 1: the plugin host costs +16.8 MiB, so the `plugins`
/// feature (on by default) is the slim-build escape hatch and must really
/// drop extism and wasmtime.
#[test]
fn slim_build_has_no_wasm_runtime() {
    let slim = tree("cox", &["--no-default-features", "--features", "otel"]);
    let full = tree("cox", &[]);
    for wasm_crate in ["cox-plugin", "extism", "wasmtime"] {
        assert!(
            !slim.contains(wasm_crate),
            "cox without `plugins` still pulls {wasm_crate}"
        );
        assert!(full.contains(wasm_crate), "default cox lacks {wasm_crate}");
    }
}

#[test]
fn no_crate_below_cox_depends_on_core() {
    let deps = workspace_deps();

    // cox-protocol is the base. Its one workspace dependency is
    // cox-plugin-api, which it re-exports as `plugin` (A52); that crate is a
    // pure leaf, so the contract still sits below every implementation.
    let protocol_allowed: HashSet<&str> = ["cox-plugin-api"].into_iter().collect();
    assert!(
        deps["cox-protocol"]
            .iter()
            .all(|d| protocol_allowed.contains(d.as_str())),
        "cox-protocol may only depend on cox-plugin-api among workspace crates, found {:?}",
        deps["cox-protocol"]
    );

    // cox-plugin-api (T33.1) builds for wasm32 so the guest SDK can use it:
    // no workspace-crate dependencies at all.
    assert!(
        deps["cox-plugin-api"].is_empty(),
        "cox-plugin-api must not depend on any other workspace crate, found {:?}",
        deps["cox-plugin-api"]
    );

    // cox-models (T30.24: the model/price catalog) depends only on
    // cox-protocol among workspace crates.
    let models_allowed: HashSet<&str> = ["cox-protocol"].into_iter().collect();
    assert!(
        deps["cox-models"]
            .iter()
            .all(|d| models_allowed.contains(d.as_str())),
        "cox-models may only depend on cox-protocol among workspace crates, found {:?}",
        deps["cox-models"]
    );

    // cox-sanitize (T32.1) is a pure trust guard, same as cox-protocol: no
    // workspace-crate dependencies at all.
    assert!(
        deps["cox-sanitize"].is_empty(),
        "cox-sanitize must not depend on any other workspace crate, found {:?}",
        deps["cox-sanitize"]
    );

    // cox-sandbox (T32.3) is a trust guard that depends only on
    // cox-protocol among workspace crates (SandboxPolicy, SandboxMode,
    // LinuxBackend, ToolError).
    let sandbox_allowed: HashSet<&str> = ["cox-protocol"].into_iter().collect();
    assert!(
        deps["cox-sandbox"]
            .iter()
            .all(|d| sandbox_allowed.contains(d.as_str())),
        "cox-sandbox may only depend on cox-protocol among workspace crates, found {:?}",
        deps["cox-sandbox"]
    );

    // cox-syntax (T32.4) is a pure parsing engine (tree-sitter and its
    // five grammars): no workspace-crate dependencies at all.
    assert!(
        deps["cox-syntax"].is_empty(),
        "cox-syntax must not depend on any other workspace crate, found {:?}",
        deps["cox-syntax"]
    );

    // cox-tokens (T32.10) is a pure leaf that depends only on cox-protocol
    // among workspace crates (ProviderError, Content, Request) — it takes a
    // plain reqwest::Client/HeaderMap rather than any cox-provider type, so
    // it never depends back on cox-provider.
    let tokens_allowed: HashSet<&str> = ["cox-protocol"].into_iter().collect();
    assert!(
        deps["cox-tokens"]
            .iter()
            .all(|d| tokens_allowed.contains(d.as_str())),
        "cox-tokens may only depend on cox-protocol among workspace crates, found {:?}",
        deps["cox-tokens"]
    );

    // cox-permission (T32.8) is a pure trust guard that depends only on
    // cox-protocol among workspace crates (rule grammar, `Engine::decide`).
    let permission_allowed: HashSet<&str> = ["cox-protocol"].into_iter().collect();
    assert!(
        deps["cox-permission"]
            .iter()
            .all(|d| permission_allowed.contains(d.as_str())),
        "cox-permission may only depend on cox-protocol among workspace crates, found {:?}",
        deps["cox-permission"]
    );

    // cox-config (T32.16), the one config owner, depends only on
    // cox-protocol among workspace crates (`Config`, `CoreError`): the CLI
    // flag layer and the `.claude/settings.json` import (cox-ext) are passed
    // in by `crates/cox`.
    let config_allowed: HashSet<&str> = ["cox-protocol"].into_iter().collect();
    assert!(
        deps["cox-config"]
            .iter()
            .all(|d| config_allowed.contains(d.as_str())),
        "cox-config may only depend on cox-protocol among workspace crates, found {:?}",
        deps["cox-config"]
    );

    // cox-telemetry (T32.9) is the otel stack: it takes plain values rather
    // than cox_protocol::Config, so it has no workspace-crate dependencies.
    assert!(
        deps["cox-telemetry"].is_empty(),
        "cox-telemetry must not depend on any other workspace crate, found {:?}",
        deps["cox-telemetry"]
    );

    // cox-core depends only on cox-protocol among workspace crates (and may
    // depend on cox-models once a card actually wires the catalog in, and
    // on cox-permission, T32.8, re-exported at the old `permission` path).
    // T34.1's `agent` tool matches a custom preset by `AgentDef`, but that
    // type lives in `cox_protocol::agent` precisely so this crate never
    // needs `cox-ext`, which does the filesystem read (`agents::discover`)
    // session assembly (`cox-session`) runs instead. cox-sanitize
    // (T33.9) holds the secret-redaction table `redact::scrub` re-exports,
    // shared with the plugin host; it is a pure leaf.
    let core_allowed: HashSet<&str> = [
        "cox-protocol",
        "cox-models",
        "cox-permission",
        "cox-sanitize",
    ]
    .into_iter()
    .collect();
    assert!(
        deps["cox-core"]
            .iter()
            .all(|d| core_allowed.contains(d.as_str())),
        "cox-core may only depend on cox-protocol/cox-models/cox-permission/cox-sanitize among workspace crates, found {:?}",
        deps["cox-core"]
    );

    // cox-tui and cox-acp may depend on cox-core, cox-protocol and
    // cox-sanitize (T32.1's guard), nothing else; cox-tui also on its
    // renderers, cox-render (T32.2); cox-acp also on the cox-sandbox guard,
    // whose `path::confine` serves an external agent's `fs/*` requests when
    // cox is its ACP client (T35.3, EA§4), and on cox-tools, whose `bash`
    // runner and `classify` serve that agent's `terminal/*` requests with no
    // second spawn path (T35.11).
    let surface_allowed: HashSet<&str> = ["cox-core", "cox-protocol", "cox-sanitize"]
        .into_iter()
        .collect();
    for crate_name in ["cox-tui", "cox-acp"] {
        let d = &deps[crate_name];
        assert!(
            d.iter().all(|dep| surface_allowed.contains(dep.as_str())
                || (crate_name == "cox-tui" && dep == "cox-render")
                || (crate_name == "cox-acp" && (dep == "cox-sandbox" || dep == "cox-tools"))),
            "{crate_name} may only depend on cox-core/cox-protocol/cox-sanitize (and cox-tui on cox-render, cox-acp on cox-sandbox and cox-tools) among workspace crates, found {d:?}"
        );
    }

    // cox-render (T32.2) is pure rendering: the protocol's config types and
    // the terminal-text guard, never the agent loop.
    let render_allowed: HashSet<&str> = ["cox-protocol", "cox-sanitize"].into_iter().collect();
    assert!(
        deps["cox-render"]
            .iter()
            .all(|dep| render_allowed.contains(dep.as_str())),
        "cox-render may only depend on cox-protocol/cox-sanitize among workspace crates, found {:?}",
        deps["cox-render"]
    );

    // cox-provider-http (T32.12) is a pure leaf shared by every wire
    // (http.rs/retry.rs/sse.rs): connection setup, credential resolution,
    // error mapping, SSE framing and retry/backoff. It depends only on
    // cox-protocol among workspace crates.
    let provider_http_allowed: HashSet<&str> = ["cox-protocol"].into_iter().collect();
    assert!(
        deps["cox-provider-http"]
            .iter()
            .all(|d| provider_http_allowed.contains(d.as_str())),
        "cox-provider-http may only depend on cox-protocol among workspace crates, found {:?}",
        deps["cox-provider-http"]
    );

    // cox-provider-openai (T32.14) is the OpenAI Responses/Chat wires and
    // the only crate that pulls in async-openai. It depends on cox-protocol,
    // cox-models (the catalog row behind effort/capabilities) and
    // cox-provider-http (transport, retry, SSE) — never back on
    // cox-provider, which re-exports it at the old `openai` path.
    let provider_openai_allowed: HashSet<&str> =
        ["cox-protocol", "cox-models", "cox-provider-http"]
            .into_iter()
            .collect();
    assert!(
        deps["cox-provider-openai"]
            .iter()
            .all(|d| provider_openai_allowed.contains(d.as_str())),
        "cox-provider-openai may only depend on cox-protocol/cox-models/cox-provider-http among workspace crates, found {:?}",
        deps["cox-provider-openai"]
    );

    // cox-provider-anthropic (T32.13) is the Anthropic Messages wire and its
    // typify build step: cox-protocol, cox-models (`effort_for`, adaptive
    // thinking) and cox-provider-http, never back on cox-provider (which
    // would cycle with cox-provider's `pub use` re-export of it).
    let provider_anthropic_allowed: HashSet<&str> =
        ["cox-protocol", "cox-models", "cox-provider-http"]
            .into_iter()
            .collect();
    assert!(
        deps["cox-provider-anthropic"]
            .iter()
            .all(|d| provider_anthropic_allowed.contains(d.as_str())),
        "cox-provider-anthropic may only depend on cox-protocol/cox-models/cox-provider-http among workspace crates, found {:?}",
        deps["cox-provider-anthropic"]
    );

    // cox-provider additionally depends on cox-models (`Priced` prices every
    // call through the catalog's `PriceTable`, T30.24), cox-tokens
    // (re-exported at the old `tokens` path, T32.10), cox-provider-http
    // (re-exported at the old `http`/`retry`/`sse` paths, T32.12),
    // cox-provider-openai (re-exported at the old `openai` path, T32.14),
    // cox-provider-testkit (T32.11): `scripted`/`replay` are thin glue over
    // the pure scenario/cassette helpers moved there, and `from_env` uses
    // them in production (COX_PROVIDER=scripted|replay), not just in tests;
    // and cox-provider-anthropic (re-exported at the old `anthropic` path,
    // T32.13).
    let provider_allowed: HashSet<&str> = [
        "cox-protocol",
        "cox-models",
        "cox-tokens",
        "cox-provider-http",
        "cox-provider-openai",
        "cox-provider-testkit",
        "cox-provider-anthropic",
    ]
    .into_iter()
    .collect();
    let provider_deps = &deps["cox-provider"];
    assert!(
        !provider_deps.contains("cox-core"),
        "cox-provider must not depend on cox-core"
    );
    assert!(
        provider_deps
            .iter()
            .all(|dep| provider_allowed.contains(dep.as_str())),
        "cox-provider may only depend on cox-protocol/cox-models/cox-tokens/cox-provider-http/cox-provider-openai/cox-provider-testkit/cox-provider-anthropic among workspace crates, found {provider_deps:?}"
    );

    // cox-provider-testkit (T32.11) is a pure leaf, same shape as
    // cox-patch/cox-sanitize/cox-syntax: no workspace-crate dependencies
    // beyond cox-protocol, so it never depends back on cox-provider (which
    // would cycle with cox-provider's `pub use` re-export of it).
    let testkit_allowed: HashSet<&str> = ["cox-protocol"].into_iter().collect();
    assert!(
        deps["cox-provider-testkit"]
            .iter()
            .all(|d| testkit_allowed.contains(d.as_str())),
        "cox-provider-testkit may only depend on cox-protocol among workspace crates, found {:?}",
        deps["cox-provider-testkit"]
    );

    // cox-patch (T32.6) is the V4A parse/match/stage engine: a pure leaf,
    // same shape as cox-models/cox-sanitize. `ApplyPatchTool` — the `Tool`
    // impl that calls `path::confine` and `write::atomic_write` — stays in
    // cox-tools so `confine` keeps its single call site.
    let patch_allowed: HashSet<&str> = ["cox-protocol"].into_iter().collect();
    assert!(
        deps["cox-patch"]
            .iter()
            .all(|d| patch_allowed.contains(d.as_str())),
        "cox-patch may only depend on cox-protocol among workspace crates, found {:?}",
        deps["cox-patch"]
    );

    // cox-search (T32.5) is the pure grep/glob walk, match and fuzzy-rank
    // engine: a pure leaf, same shape as cox-patch/cox-syntax. `GrepTool`/
    // `GlobTool` — the `Tool` impls that call `path::confine` and, for
    // `grep`, archive over-cap results — stay in cox-tools so `confine`
    // keeps its single call site.
    assert!(
        deps["cox-search"].is_empty(),
        "cox-search must not depend on any other workspace crate, found {:?}",
        deps["cox-search"]
    );

    // cox-web (T32.7) is the fetch/extract engine behind `web_fetch`: a
    // pure leaf, same shape as cox-patch. `WebFetchTool` — the `Tool` impl
    // that owns `ToolCx` and input parsing — stays in cox-tools.
    let web_allowed: HashSet<&str> = ["cox-protocol"].into_iter().collect();
    assert!(
        deps["cox-web"]
            .iter()
            .all(|d| web_allowed.contains(d.as_str())),
        "cox-web may only depend on cox-protocol among workspace crates, found {:?}",
        deps["cox-web"]
    );

    // mcp/store/ext depend only on cox-protocol: this is the rule the test
    // is named for — none of them may reach cox-core. cox-ext also takes
    // the leaf cox-sanitize (T44.2: presence text names another session's
    // worktree to the model through the one guard).
    for crate_name in ["cox-mcp", "cox-store", "cox-ext"] {
        let leaf_allowed: HashSet<&str> = match crate_name {
            "cox-ext" => ["cox-protocol", "cox-sanitize"].into_iter().collect(),
            _ => ["cox-protocol"].into_iter().collect(),
        };
        let d = &deps[crate_name];
        assert!(
            !d.contains("cox-core"),
            "{crate_name} must not depend on cox-core"
        );
        assert!(
            d.iter().all(|dep| leaf_allowed.contains(dep.as_str())),
            "{crate_name} may only depend on {leaf_allowed:?} among workspace crates, found {d:?}"
        );
    }

    // cox-tools additionally depends on cox-sandbox (T32.3: path::confine
    // and the sandbox backends), cox-patch (T32.6: the V4A engine),
    // cox-syntax (T32.4: outline and parse_bash), cox-search (T32.5: the
    // grep/glob walk and match engine) and cox-web (T32.7: the web_fetch
    // engine).
    let tools_allowed: HashSet<&str> = [
        "cox-protocol",
        "cox-sandbox",
        "cox-patch",
        "cox-syntax",
        "cox-search",
        "cox-web",
    ]
    .into_iter()
    .collect();
    let tools_deps = &deps["cox-tools"];
    assert!(
        !tools_deps.contains("cox-core"),
        "cox-tools must not depend on cox-core"
    );
    assert!(
        tools_deps
            .iter()
            .all(|dep| tools_allowed.contains(dep.as_str())),
        "cox-tools may only depend on cox-protocol/cox-sandbox/cox-patch/cox-syntax/cox-search/cox-web among workspace crates, found {tools_deps:?}"
    );
}

/// T37.1 (DT§4.2): session assembly is a library every surface shares, so
/// it carries no CLI parser, no `anyhow` and no terminal: the flags stay in
/// `crates/cox`, errors are `SessionError`, warnings come back as data.
#[test]
fn session_has_no_cli_or_terminal() {
    let deps = &all_deps()["cox-session"];
    for banned in ["clap", "anyhow", "cox-tui"] {
        assert!(
            !deps.contains(banned),
            "cox-session must not depend on {banned}"
        );
    }
}

/// T37.8 (DT§4.2): the application core is UI-agnostic — the desktop app
/// links it through `cox-ffi`, so no terminal toolkit may reach it, not even
/// through `cox-render`'s default `ratatui` feature. D1 bans *depending on*
/// `anyhow` and `clap`, so those are checked on its own manifest (T37.39):
/// `cox-session` pulls `anyhow` transitively (tiktoken-rs, the ACP crate).
#[test]
fn app_has_no_terminal_or_cli() {
    let deps = tree("cox-app", &[]);
    for banned in ["ratatui", "crossterm", "cox-tui"] {
        assert!(!deps.contains(banned), "cox-app must not pull {banned}");
    }
    let direct = &all_deps()["cox-app"];
    for banned in ["clap", "anyhow"] {
        assert!(
            !direct.contains(banned),
            "cox-app must not depend on {banned}"
        );
    }
}

/// T37.39 (D11, DT§4.2): the FFI only forwards — sessions are `cox-app`'s —
/// so among workspace crates it reaches `cox-app` and `cox-protocol` alone.
#[test]
fn ffi_depends_only_on_app_and_protocol() {
    let deps = &workspace_deps()["cox-ffi"];
    let expected: HashSet<String> = ["cox-app", "cox-protocol"].map(String::from).into();
    assert_eq!(deps, &expected, "cox-ffi's direct workspace dependencies");
}

/// T37.14 (D1, DT§4.2): the app's FFI layer is the one crate built on
/// UniFFI, and the `cox` binary does not link it.
#[test]
fn only_ffi_depends_on_uniffi() {
    for (crate_name, crate_deps) in &all_deps() {
        if crate_name != "cox-ffi" {
            assert!(
                !crate_deps.contains("uniffi"),
                "{crate_name} must not depend on uniffi; only cox-ffi does"
            );
        }
    }
    assert!(all_deps()["cox-ffi"].contains("uniffi"));
    let cli = tree("cox", &[]);
    for banned in ["cox-ffi", "uniffi"] {
        assert!(
            !cli.contains(banned),
            "the cox binary must not pull {banned}"
        );
    }
}
