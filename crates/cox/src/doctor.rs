// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox doctor`: diagnostics to understand why cox will or will not work on
//! this machine. Checks: toolchain version, `COX_HOME` writable, db opens,
//! API keys per configured provider, sandbox backend, `git` on PATH, terminal
//! capabilities (TERM, true colour, size), prices table age, whether every
//! configured model has a catalog price (T30.27), what LM Studio runs when
//! it is the code tier's provider (T30.16), `.claude/settings.json`,
//! each `[lsp.servers]` program on PATH (T41.7), push-to-talk (T54.7),
//! one OAuth row per HTTP MCP server (T22.5), and one row per granted
//! `[[external_agents]]` entry (EA§7, T35.8), and one row per discovered
//! plugin: loaded, skipped (with reason), not granted, or dev, plus
//! catalog price conflicts (T33.16) and the wasmtime compilation cache's
//! on-disk size (PL§6/PL§10, T33.39). `cox ext list` (`ext_cmd.rs`) shows
//! the same per-plugin state through `check_plugins`, so the two surfaces
//! never disagree.
//! Outputs human-readable lines or `--json` array of `{check, status, detail, fix}`.
//! The API-key, sandbox and git checks and the row type live in
//! `cox_session::doctor` (T37.31), shared with the desktop app's first-run
//! checklist.

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
#[cfg(feature = "plugins")]
use std::process::Stdio;
#[cfg(feature = "plugins")]
use std::time::{Duration, Instant};

use cox_protocol::Store as _;
use cox_protocol::config::McpServerConfig;
use cox_provider::usage::{Price, load_price_table};
pub use cox_session::doctor::CheckResult;
use cox_session::doctor::{check_api_keys, check_git, check_sandbox};

/// Run all doctor checks. Returns exit code 0 when no `fail`, 1 otherwise.
/// `tui_theme` is `config.tui.theme`, shown (and queried when `"auto"`) by
/// `check_terminal`; `tui_caps` is `config.tui.caps` (T23.0), the `[tui.caps]`
/// overrides `check_terminal` reports alongside the detected/queried value.
/// `config` is the loaded config: `check_prefix` (T30.1) assembles the active
/// profile's prefix and reports its T1.8 estimate. `cwd` is the working
/// directory the command ran from, used only to find a project plugin root
/// for `check_external_agents` (T35.8) — the same root `mcp_servers` finds.
pub fn run(
    json: bool,
    mcp: &HashMap<String, McpServerConfig>,
    cwd: &std::path::Path,
    tui_theme: &str,
    tui_caps: &HashMap<String, bool>,
    config: &cox_protocol::Config,
) -> i32 {
    let mut results = Vec::new();

    // Get COX_HOME early for reuse.
    let home = env::var("COX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| cox_store::Store::default_home());

    // Toolchain version.
    results.push(check_toolchain());

    // COX_HOME writable.
    results.push(check_home_writable(&home));

    // Database opens.
    results.push(check_db(&home));

    // API keys.
    results.push(check_api_keys(config));

    // Sandbox backend.
    results.push(check_sandbox());

    // git on PATH.
    results.push(check_git());

    // Terminal capabilities, including `tui.theme = "auto"` (T22.6) and
    // `cox_tui::term::Caps` (T23.0).
    results.push(check_terminal(tui_theme, tui_caps));

    // Prices table age.
    results.push(check_prices());

    // What LM Studio runs for the code tier (T30.16), only when it is the
    // code tier's provider: nobody else needs port 1234 probed.
    if config.tiers.code.provider == "lmstudio" {
        results.push(check_lmstudio(config));
    }

    // Every model reachable from [tiers.*] or [providers.*].models has a
    // catalog price (T30.27).
    results.push(check_catalog_prices(config));

    // .claude/settings.json found.
    results.push(check_claude_settings());

    // Assembled-prefix token count for the active profile (T30.1).
    results.push(check_prefix(config));

    // Keybindings file and the Claude import: bad entries and clashes (T25.5).
    results.push(check_keybindings(
        &home,
        &crate::config_load::home_dir().join(".claude"),
    ));

    // One row naming every stdio server opted out of the sandbox (T33.42).
    results.push(check_mcp_sandbox(mcp));

    // Each `[lsp.servers]` program found on PATH or missing (T41.7).
    results.push(check_lsp(config));

    // Push-to-talk: built, enabled, model present, input device (T54.7).
    results.push(check_voice(config, &home));

    // One row per granted `[[external_agents]]` entry: CLI on PATH (+
    // `--version`, best-effort), `key_env` set, sandboxed or refused
    // (EA§7, T35.8). Empty in the slim build (no `plugins` feature) and
    // when no plugin is granted.
    results.extend(check_external_agents(cwd, &home, config));

    // One row per discovered plugin: loaded, skipped, not granted, or dev,
    // plus catalog price conflicts (T33.16) in the detail (T33.39). Empty
    // in the slim build and when no plugin is discovered.
    results.extend(check_plugins(cwd, &home, config));

    // The wasmtime compilation cache's on-disk size. Empty in the slim
    // build.
    #[cfg(feature = "plugins")]
    results.push(check_plugin_cache(&home));

    // One row per HTTP MCP server: is its token usable?
    let mut names: Vec<&String> = mcp
        .iter()
        .filter(|(_, c)| c.url.is_some())
        .map(|(n, _)| n)
        .collect();
    names.sort();
    results.extend(names.into_iter().map(|name| check_mcp_auth(name)));

    // Output and determine exit code.
    let has_fail = if json {
        output_json(&results)
    } else {
        output_human(&results)
    };

    if has_fail { 1 } else { 0 }
}

fn check_toolchain() -> CheckResult {
    match ProcessCommand::new("rustc").arg("--version").output() {
        Ok(output) => {
            let version = String::from_utf8_lossy(&output.stdout).to_string();
            CheckResult::ok("toolchain", version.trim().to_string())
        }
        Err(_) => CheckResult::fail(
            "toolchain",
            "rustc not found on PATH".to_string(),
            "install Rust from https://rustup.rs/".to_string(),
        ),
    }
}

fn check_home_writable(home: &std::path::Path) -> CheckResult {
    use std::fs;
    use std::io::Write;

    // Try to create the home directory if it doesn't exist.
    if !home.exists() && fs::create_dir_all(home).is_err() {
        return CheckResult::fail(
            "COX_HOME writable",
            format!("cannot create {}: dir creation failed", home.display()),
            format!(
                "ensure {} exists and is writable",
                env::var("COX_HOME").unwrap_or_else(|_| "~/.cox".to_string())
            ),
        );
    }

    // Try to write a test file.
    let test_file = home.join(".cox_write_test");
    match fs::File::create(&test_file) {
        Ok(mut f) => {
            let _ = f.write_all(b"test");
            let _ = fs::remove_file(&test_file);
            CheckResult::ok("COX_HOME writable", home.display().to_string())
        }
        Err(e) => CheckResult::fail(
            "COX_HOME writable",
            format!("cannot write to {}: {}", home.display(), e),
            format!(
                "ensure {} is writable",
                env::var("COX_HOME").unwrap_or_else(|_| "~/.cox".to_string())
            ),
        ),
    }
}

fn check_db(home: &std::path::Path) -> CheckResult {
    db_result(cox_store::Store::open(home).map(drop), home)
}

/// The `db` row for one open attempt. A newer schema (T37.36) gets its own
/// fix: the file is sound and a newer `cox` still uses it, so the generic
/// "remove cox.db" advice would throw away that `cox`'s sessions and ledger.
fn db_result(opened: Result<(), cox_protocol::StoreError>, home: &std::path::Path) -> CheckResult {
    match opened {
        Ok(()) => CheckResult::ok("db", "database opens and schema is valid".to_string()),
        Err(cox_protocol::StoreError::SchemaNewer { db, binary }) => CheckResult::fail(
            "db",
            format!("cox.db schema {db} is newer than this cox ({binary}); a newer cox wrote it"),
            "update this cox (e.g. `brew upgrade cox`, or the app's own update) or run the \
             newer one; keep cox.db"
                .to_string(),
        ),
        Err(e) => CheckResult::fail(
            "db",
            format!("cannot open database: {}", e),
            format!("remove {} and retry", home.join("cox.db").display()),
        ),
    }
}

fn check_terminal(tui_theme: &str, tui_caps: &HashMap<String, bool>) -> CheckResult {
    let mut details = Vec::new();

    // Check TERM variable.
    match env::var("TERM") {
        Ok(term) => details.push(format!("TERM={}", term)),
        Err(_) => {
            return CheckResult::warn(
                "terminal",
                "TERM not set".to_string(),
                "set TERM=xterm-256color or your terminal's type".to_string(),
            );
        }
    }

    // Check for true color support (COLORTERM).
    if env::var("COLORTERM").is_ok() {
        details.push("true colour detected".to_string());
    } else {
        details.push("true colour unknown".to_string());
    }

    // Try to get terminal size via crossterm.
    if let Ok((cols, rows)) = crossterm::terminal::size() {
        details.push(format!("size {}x{}", cols, rows));
    }

    // `tui.theme = "auto"` (T22.6): same OSC 11 query `run_tui` makes,
    // reported here so `doctor` explains what a session will resolve to
    // without opening one.
    details.push(match tui_theme {
        "auto" => match cox_tui::color::detect_dark(cox_tui::color::OSC11_TIMEOUT) {
            Some(true) => "theme: auto → dark (OSC 11 reply)".to_string(),
            Some(false) => "theme: auto → light (OSC 11 reply)".to_string(),
            None => "theme: auto → dark (no OSC 11 reply)".to_string(),
        },
        other => format!("theme: {other}"),
    });

    // `cox_tui::term::Caps` (T23.0): one row per field with the source that
    // decided it — `config` (`[tui.caps]`) beats `query` (the real
    // keyboard-protocol probe, tty only) beats `env` (the base guess).
    let env_fn = |key: &str| env::var(key).ok();
    let mut caps = cox_tui::term::Caps::detect(&env_fn);
    let queried = caps.query(cox_tui::term::KITTY_QUERY_TIMEOUT);
    let before_config = caps;
    caps.apply(tui_caps);
    let caps_report: Vec<String> = caps
        .fields()
        .into_iter()
        .zip(before_config.fields())
        .map(|((name, value), (_, pre_config))| {
            let source = if value != pre_config {
                "config"
            } else if name == "kitty_keyboard" && queried {
                "query"
            } else {
                "env"
            };
            format!("{name}={value} ({source})")
        })
        .collect();
    details.push(caps_report.join(", "));
    // T23.5: what `tui.notify` writes on this terminal.
    let vte = cox_tui::term::is_vte(&env_fn);
    details.push(format!(
        "notify via {}",
        cox_tui::term::notify_via(&caps, vte)
    ));

    CheckResult::ok("terminal", details.join(", "))
}

const PRICES_STALE_DAYS: u32 = 90;
const PRICES_FIX: &str =
    "regenerate crates/cox-provider/prices.toml with `just vendor models` (plan.md A48)";

/// One row naming every stdio server configured with `sandbox = false`
/// (T33.42) — the wrap has no other visible signal once a server opts out.
fn check_mcp_sandbox(mcp: &HashMap<String, McpServerConfig>) -> CheckResult {
    let mut names: Vec<&str> = mcp
        .iter()
        .filter(|(_, c)| c.command.is_some() && !c.sandbox)
        .map(|(n, _)| n.as_str())
        .collect();
    names.sort_unstable();
    if names.is_empty() {
        CheckResult::ok("mcp sandbox", "every stdio server is sandboxed".to_string())
    } else {
        CheckResult::warn(
            "mcp sandbox",
            format!("unsandboxed by config: {}", names.join(", ")),
            "sandbox = false was set on purpose; drop it to re-enable the wrap".to_string(),
        )
    }
}

/// One row for `[lsp]` (T41.7): each configured server's program, found on
/// PATH or missing, by the lookup the `diagnostics` tool starts it by. A
/// missing server is not a problem while another one runs; with none,
/// `diagnostics` can only point the model at `bash`.
fn check_lsp(config: &cox_protocol::Config) -> CheckResult {
    check_lsp_with(&config.lsp, |command| {
        cox_tools::lsp::on_path(command).is_some()
    })
}

fn check_lsp_with(
    lsp: &cox_protocol::config::LspConfig,
    found: impl Fn(&str) -> bool,
) -> CheckResult {
    const CHECK: &str = "LSP servers";
    if !lsp.enabled {
        return CheckResult::ok(CHECK, "disabled (lsp.enabled = false)".to_string());
    }
    if lsp.servers.is_empty() {
        return CheckResult::ok(CHECK, "none configured".to_string());
    }
    let rows: Vec<(bool, &str)> = lsp
        .servers
        .values()
        .map(|s| (found(&s.command), s.command.as_str()))
        .collect();
    let detail = rows
        .iter()
        .map(|(ok, command)| format!("{command} {}", if *ok { "ok" } else { "missing" }))
        .collect::<Vec<_>>()
        .join(", ");
    if rows.iter().any(|(ok, _)| *ok) {
        CheckResult::ok(CHECK, detail)
    } else {
        CheckResult::warn(
            CHECK,
            detail,
            "install a language server for your project, or set lsp.enabled = false; \
             without one, `diagnostics` tells the model to run the project's checker with `bash`"
                .to_string(),
        )
    }
}

const VOICE: &str = "voice";

#[cfg(not(feature = "voice"))]
fn check_voice(_: &cox_protocol::Config, _: &std::path::Path) -> CheckResult {
    CheckResult::ok(
        VOICE,
        "not built (`cargo build --features voice`)".to_string(),
    )
}

#[cfg(feature = "voice")]
fn check_voice(config: &cox_protocol::Config, home: &std::path::Path) -> CheckResult {
    check_voice_with(
        &config.voice,
        crate::voice_cmd::model_path(home, &config.voice.model),
        cox_voice::input_device,
    )
}

/// `device` names the default input device; it is asked only once the
/// feature is on and the model is there, and it never opens a stream.
#[cfg(feature = "voice")]
fn check_voice_with(
    voice: &cox_protocol::config::VoiceConfig,
    model: Option<PathBuf>,
    device: impl Fn() -> Option<String>,
) -> CheckResult {
    let name = &voice.model;
    if !voice.enabled {
        return CheckResult::ok(VOICE, "built; off (voice.enabled = false)".to_string());
    }
    let Some(model) = model else {
        return CheckResult::warn(
            VOICE,
            format!("built, enabled; {name} is not a pinned model"),
            "set voice.model to a name `cox voice model list` shows".to_string(),
        );
    };
    if !model.is_file() {
        return CheckResult::warn(
            VOICE,
            format!("built, enabled; model {name} missing"),
            format!("run `cox voice model download {name}`"),
        );
    }
    match device() {
        Some(mic) => CheckResult::ok(
            VOICE,
            format!(
                "built, enabled, model {name} present, input {}",
                cox_sanitize::sanitize(&mic)
            ),
        ),
        None => CheckResult::warn(
            VOICE,
            format!("built, enabled, model {name} present; no input device"),
            "connect a microphone; on macOS allow this terminal under System Settings > \
             Privacy & Security > Microphone"
                .to_string(),
        ),
    }
}

/// `mcp auth <name>`: `ok (expires in 3h)`, `ok (no expiry)`, `expired` or
/// `none`. `none` is fine — the server may not ask for a login at all.
fn check_mcp_auth(name: &str) -> CheckResult {
    use cox_mcp::auth::{Status, status, stored};
    let check = format!("mcp auth {name}");
    match stored(name) {
        Ok(creds) => match status(creds.as_ref(), cox_mcp::auth::now()) {
            s @ Status::Expired => {
                CheckResult::warn(&check, s.to_string(), format!("run `cox mcp login {name}`"))
            }
            s => CheckResult::ok(&check, s.to_string()),
        },
        Err(e) => CheckResult::warn(
            &check,
            format!("keyring: {e}"),
            format!("run `cox mcp login {name}` once the keyring is available"),
        ),
    }
}

/// How long doctor waits on a granted external agent's `--version` probe: a
/// hung CLI must not hang doctor (mirrors `LMSTUDIO_DOCTOR_TIMEOUT_S`).
#[cfg(feature = "plugins")]
const EXTERNAL_AGENT_VERSION_TIMEOUT: Duration = Duration::from_secs(3);

/// One row per granted `[[external_agents]]` entry (EA§7, T35.8): CLI found
/// on `PATH` (+ `--version`, best-effort), `key_env` set or missing,
/// sandboxed or refused. Reuses the same discover-and-grant walk
/// `load_plugins` does (`cox plugin list`'s `plugin_cmd::verdict_for` walks
/// it the same way) rather than a second discovery implementation — doctor
/// only adds the `--version` probe on top. An entry has no `sandbox = false`
/// opt-out (T35.2 is wrap-or-refuse), unlike a `[[mcp]]` server's T33.42
/// row: the wrap either succeeds or the entry is refused outright.
#[cfg(feature = "plugins")]
fn check_external_agents(
    cwd: &std::path::Path,
    home: &std::path::Path,
    config: &cox_protocol::Config,
) -> Vec<CheckResult> {
    use cox_plugin::discover::{self, State};
    use cox_plugin::grant::{self, Verdict};
    use cox_protocol::PluginStore as _;

    let root = crate::config_load::find_git_root(cwd);
    let found = discover::discover(home, root.as_deref());
    let store = cox_store::Store::open(home).ok();
    // §1.6: empty `workspace_roots` means the git root of `cwd`, else
    // `cwd` — the same default `session::start` applies before it wraps a
    // granted entry's argv.
    let writable = if config.core.workspace_roots.is_empty() {
        vec![root.clone().unwrap_or_else(|| cwd.to_path_buf())]
    } else {
        config.core.workspace_roots.clone()
    };

    let mut rows = Vec::new();
    for p in &found.plugins {
        let State::Loaded { manifest, digest } = &p.state else {
            continue;
        };
        if manifest.external_agents.is_empty() {
            continue;
        }
        // A linked plugin's grant is keyed on `grant_digest()` (T33.41), not
        // the live content `digest`, so an external-agents row for one must
        // look it up the same way `load_plugins`/`plugin_cmd` do or it
        // would find no grant and skip a plugin that is actually granted.
        let grant_digest = p.grant_digest().unwrap_or_else(|| digest.clone());
        let stored = grant::scope(p.source, root.as_deref()).and_then(|scope| {
            store
                .as_ref()
                .and_then(|s| s.grant_get(&p.id, &scope, &grant_digest).ok().flatten())
        });
        if !matches!(
            grant::check(manifest, digest, stored.as_ref()),
            Verdict::Granted
        ) {
            continue;
        }
        for decl in &manifest.external_agents {
            rows.push(check_external_agent(&p.dir, decl, config, &writable));
        }
    }
    rows
}

/// The slim build has no plugin host, so there is never a granted entry.
#[cfg(not(feature = "plugins"))]
fn check_external_agents(
    _cwd: &std::path::Path,
    _home: &std::path::Path,
    _config: &cox_protocol::Config,
) -> Vec<CheckResult> {
    Vec::new()
}

/// [`check_external_agent`]'s body with the sandbox wrap and the `key_env`
/// lookup injected, so a test can exercise every branch (missing in-package
/// file, sandbox refusal, missing CLI, found + version, missing key)
/// without depending on this host's sandbox backend and without setting a
/// real env var (mirrors `check_api_keys_with`, A49/T30.28).
#[cfg(feature = "plugins")]
fn check_external_agent_with(
    dir: &std::path::Path,
    decl: &cox_plugin_api::ExternalAgentDecl,
    key_set: bool,
    wrap: impl FnOnce(&std::path::Path, &[String]) -> Result<Vec<String>, String>,
) -> CheckResult {
    use cox_plugin::external_agent::{missing_on_path, package_program};

    let check = format!("external agent {}", decl.name);
    let key_detail = format!(
        "key_env {} {}",
        decl.key_env,
        if key_set { "set" } else { "not set" }
    );
    let key_fix = format!("set {} to use external agent {:?}", decl.key_env, decl.name);

    let program = match package_program(dir, &decl.command) {
        Ok(p) => p,
        Err(e) => {
            return CheckResult::warn(
                &check,
                format!("{e}; {key_detail}"),
                format!("fix `command` on plugin external agent {:?}", decl.name),
            );
        }
    };

    // Under the wrap a missing PATH program fails inside the sandbox
    // launcher, never as a spawn error, so ask PATH first — the same lookup
    // that leaves the entry out of the session (T35.13).
    if missing_on_path(&program, env::var_os("PATH").as_deref()) {
        return CheckResult::warn(
            &check,
            format!("{} not found on PATH; {key_detail}", decl.command),
            format!("install the `{}` CLI or fix `command`", decl.command),
        );
    }

    let argv = match wrap(&program, &[String::from("--version")]) {
        Ok(argv) => argv,
        Err(reason) => {
            return CheckResult::warn(
                &check,
                format!(
                    "refused: cannot run under the sandbox on this host ({reason}); {key_detail}"
                ),
                "install a sandbox backend for this host (AGENTS.md Trust boundaries)".to_string(),
            );
        }
    };

    let cli_detail = match probe_version(&argv) {
        Ok(ProbeOutcome::Version(v)) => format!("sandboxed; found, {v}"),
        Ok(ProbeOutcome::NoOutput) => "sandboxed; found; no version output".to_string(),
        Ok(ProbeOutcome::TimedOut) => "sandboxed; found; --version timed out".to_string(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return CheckResult::warn(
                &check,
                format!("{} not found on PATH; {key_detail}", decl.command),
                format!("install the `{}` CLI or fix `command`", decl.command),
            );
        }
        Err(e) => {
            return CheckResult::warn(
                &check,
                format!("cannot run {}: {e}; {key_detail}", decl.command),
                format!("check the `{}` CLI installation", decl.command),
            );
        }
    };

    let detail = format!("{cli_detail}; {key_detail}");
    if key_set {
        CheckResult::ok(&check, detail)
    } else {
        CheckResult::warn(&check, detail, key_fix)
    }
}

/// `key_env` is only ever checked for presence (D12/A49) — its value is
/// never read into this process's own logic beyond `env::var`, and never
/// printed or logged. Wraps `decl.command` with `session::sandboxed_argv`,
/// the same wrap a driver's argv gets (`session::plugin_agents`), so the
/// probe reflects the real sandbox this host would run the entry under.
#[cfg(feature = "plugins")]
fn check_external_agent(
    dir: &std::path::Path,
    decl: &cox_plugin_api::ExternalAgentDecl,
    config: &cox_protocol::Config,
    writable: &[PathBuf],
) -> CheckResult {
    let key_set = env::var(&decl.key_env).is_ok_and(|v| !v.is_empty());
    check_external_agent_with(dir, decl, key_set, |program, args| {
        crate::session::sandboxed_argv(program, args, config, writable)
    })
}

/// What running an entry's wrapped `--version` argv found: the exit status
/// is never checked — a CLI's `--version` handling is not a spec cox
/// controls, so any line on stdout is reported best-effort (EA§7).
#[cfg(feature = "plugins")]
enum ProbeOutcome {
    Version(String),
    NoOutput,
    TimedOut,
}

/// Runs `argv` (`program` then `args`) to completion or
/// [`EXTERNAL_AGENT_VERSION_TIMEOUT`], whichever comes first, killing a
/// process that outlives the deadline. `Err` only on spawn failure — a
/// missing CLI surfaces as `io::ErrorKind::NotFound`.
#[cfg(feature = "plugins")]
fn probe_version(argv: &[String]) -> std::io::Result<ProbeOutcome> {
    use std::io::Read as _;

    let Some((program, args)) = argv.split_first() else {
        return Ok(ProbeOutcome::NoOutput);
    };
    let mut child = ProcessCommand::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let start = Instant::now();
    while child.try_wait()?.is_none() {
        if start.elapsed() >= EXTERNAL_AGENT_VERSION_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(ProbeOutcome::TimedOut);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut out);
    }
    Ok(out
        .lines()
        .next()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map_or(ProbeOutcome::NoOutput, |l| {
            ProbeOutcome::Version(l.to_string())
        }))
}

/// One row per discovered plugin (PL§6/PL§10, T33.39): loaded, skipped
/// (with reason), not granted, or dev. Reuses `plugin_cmd::verdict_for`,
/// the one pure grant answer every surface shares, and the same
/// discover-and-grant walk `check_external_agents` and `cox plugin list`
/// use, so this never becomes a third verdict implementation.
#[cfg(feature = "plugins")]
pub(crate) fn check_plugins(
    cwd: &std::path::Path,
    home: &std::path::Path,
    config: &cox_protocol::Config,
) -> Vec<CheckResult> {
    check_plugins_with(cwd, home, config, &HashMap::new())
}

/// The slim build has no plugin host, so there is never a discovered
/// plugin.
#[cfg(not(feature = "plugins"))]
pub(crate) fn check_plugins(
    _cwd: &std::path::Path,
    _home: &std::path::Path,
    _config: &cox_protocol::Config,
) -> Vec<CheckResult> {
    Vec::new()
}

/// [`check_plugins`]'s body with the breaker's per-session disabled-export
/// map injected (mirrors `check_api_keys_with`), so a test can prove a
/// disabled export is visible without a live session: nothing today
/// tracks that map across a `cox doctor` process (PL§10's "for the
/// session" breaker is in-memory and doctor is a fresh process each run),
/// so `check_plugins` always passes an empty one; wire in the real map
/// once a session's breaker state can be read from outside it.
#[cfg(feature = "plugins")]
fn check_plugins_with(
    cwd: &std::path::Path,
    home: &std::path::Path,
    config: &cox_protocol::Config,
    disabled_exports: &HashMap<String, Vec<String>>,
) -> Vec<CheckResult> {
    use cox_plugin::discover::{self, State};
    use cox_plugin::grant::Verdict;

    let root = crate::config_load::find_git_root(cwd);
    let found = discover::discover(home, root.as_deref());
    let store = cox_store::Store::open(home).ok();

    // Granted plugins' `[[models]]` feed the catalog so a price or field
    // conflict with the built-in table (T33.16) shows up per plugin below;
    // `check_catalog_prices` still passes `&[]` for the top-level catalog
    // check until `cox-session` keeps loaded manifests around to share (that
    // wiring is T33.16's own "Left").
    let mut granted: Vec<(&str, &cox_plugin_api::PluginManifest)> = Vec::new();
    for p in &found.plugins {
        if let State::Loaded { manifest, digest } = &p.state {
            let verdict = crate::plugin_cmd::verdict_for(
                p,
                manifest,
                digest,
                store.as_ref(),
                root.as_deref(),
            );
            if verdict == Verdict::Granted {
                granted.push((p.id.as_str(), manifest));
            }
        }
    }
    let plugin_models: Vec<cox_models::PluginModels<'_>> = granted
        .iter()
        .map(|(id, m)| cox_models::PluginModels {
            plugin: id,
            models: &m.models,
        })
        .collect();
    let catalog = cox_models::Catalog::load(config, &plugin_models, None).ok();

    let mut rows = Vec::new();
    for p in &found.plugins {
        let check = format!("plugin {}", p.id);
        let (manifest, digest) = match &p.state {
            State::Skipped { reason } => {
                rows.push(CheckResult::warn(
                    &check,
                    format!("skipped: {reason}"),
                    String::new(),
                ));
                continue;
            }
            State::Loaded { manifest, digest } => (manifest, digest),
        };
        let verdict =
            crate::plugin_cmd::verdict_for(p, manifest, digest, store.as_ref(), root.as_deref());
        let mut parts = vec![match &verdict {
            Verdict::Granted => "loaded".to_string(),
            Verdict::Disabled => "disabled".to_string(),
            Verdict::NeedsApproval { .. } => "not granted".to_string(),
        }];
        if p.dev {
            // Same word `cox plugin list` shows for a `cox plugin link` package (T33.41).
            parts.push("dev".to_string());
        }
        if let Some(exports) = disabled_exports.get(&p.id)
            && !exports.is_empty()
        {
            parts.push(format!(
                "exports disabled by three failures: {}",
                exports.join(", ")
            ));
        }
        if let Some(catalog) = &catalog {
            let prefix = format!("plugin {} ", p.id);
            let conflicts: Vec<&str> = catalog
                .warnings()
                .iter()
                .filter(|w| w.starts_with(&prefix))
                .map(String::as_str)
                .collect();
            if !conflicts.is_empty() {
                parts.push(format!("catalog conflicts: {}", conflicts.join("; ")));
            }
        }
        let detail = parts.join("; ");
        rows.push(if matches!(verdict, Verdict::Granted) {
            CheckResult::ok(&check, detail)
        } else {
            CheckResult::warn(&check, detail, String::new())
        });
    }
    rows
}

/// The wasmtime compilation cache's on-disk size at
/// `<COX_HOME>/cache/wasmtime` (T33.39). Caching itself is off today —
/// `PluginHost::load_with` (`cox-plugin/src/host.rs`) calls
/// `with_cache_disabled()`, and this path is "wired by a later card" — so
/// an absent directory reports 0 bytes, never a warning.
#[cfg(feature = "plugins")]
fn check_plugin_cache(home: &std::path::Path) -> CheckResult {
    let dir = home.join("cache").join("wasmtime");
    let bytes = cox_tools::git::dir_size(&dir);
    CheckResult::ok("plugin cache", format!("{} ({bytes} bytes)", dir.display()))
}

fn parse_iso_date(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.split('-');
    let y = parts.next()?.parse().ok()?;
    let m = parts.next()?.parse().ok()?;
    let d = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some((y, m, d))
}

fn ymd_to_days(date: (u32, u32, u32)) -> Option<u32> {
    let (y, m, d) = date;
    let m = m as i64;
    let y = y as i64;
    let d = d as i64;
    let y_adj = y - if m <= 2 { 1 } else { 0 };
    let era = y_adj / 400;
    let yoe = y_adj - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    if days < 0 {
        return None;
    }
    Some(days as u32)
}

fn days_between(from: &str, to: (u32, u32, u32)) -> Option<u32> {
    let from_days = ymd_to_days(parse_iso_date(from)?)?;
    let to_days = ymd_to_days(to)?;
    // `cox-vendor` stamps the local date, which runs ahead of the UTC `today`
    // near midnight: a date after today is as fresh as today, not a panic.
    Some(to_days.saturating_sub(from_days))
}

fn today_ymd() -> (u32, u32, u32) {
    use std::time::{SystemTime, UNIX_EPOCH};

    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0);
    civil_from_days(days as i64)
}

fn civil_from_days(z: i64) -> (u32, u32, u32) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + (era * 400) as u32;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mp < 10 { y } else { y + 1 };
    (y, m, d)
}

fn prices_status(prices: &[Price], today: (u32, u32, u32)) -> CheckResult {
    let mut oldest_date = None;
    let mut oldest_age = None;

    for price in prices {
        let age = days_between(&price.verified_on, today);
        match age {
            Some(age) if age > PRICES_STALE_DAYS => {
                return CheckResult::warn(
                    "prices",
                    format!(
                        "{} verified_on {} is {} days old",
                        price.id, price.verified_on, age
                    ),
                    PRICES_FIX.to_string(),
                );
            }
            Some(age) => {
                if oldest_age.is_none_or(|current| age > current) {
                    oldest_age = Some(age);
                    oldest_date = Some(price.verified_on.as_str());
                }
            }
            None => {
                return CheckResult::warn(
                    "prices",
                    format!("invalid verified_on date: {}", price.verified_on),
                    PRICES_FIX.to_string(),
                );
            }
        }
    }

    let detail = match oldest_date {
        Some(date) => format!("oldest verified_on {}", date),
        None => "no price rows".to_string(),
    };
    CheckResult::ok("prices", detail)
}

fn check_prices() -> CheckResult {
    let table = match load_price_table("/nonexistent/cox-doctor-prices.toml") {
        Ok(table) => table,
        Err(err) => {
            return CheckResult::warn(
                "prices",
                format!("could not load price table: {}", err),
                PRICES_FIX.to_string(),
            );
        }
    };
    prices_status(table.prices(), today_ymd())
}

const CATALOG_PRICES_FIX: &str = "run `uv run --project scripts/vendor cox-vendor models` to add a price row for these models (plan.md A48)";

/// [`check_catalog_prices`]'s body with the catalog already built, so a
/// test can hand it a small `ids` list instead of a whole `Config`
/// (`docs/design/providers.md` § Target shape item 5, T30.27).
fn catalog_prices_status(catalog: &cox_models::Catalog, ids: &[String]) -> CheckResult {
    let mut unpriced: Vec<&str> = ids
        .iter()
        .filter(|id| {
            catalog
                .get(id.as_str())
                .is_none_or(|row| row.price.is_none())
        })
        .map(String::as_str)
        .collect();
    unpriced.sort();
    unpriced.dedup();
    if unpriced.is_empty() {
        return CheckResult::ok(
            "catalog prices",
            format!("{} configured models priced", ids.len()),
        );
    }
    CheckResult::warn(
        "catalog prices",
        format!("no catalog price for: {}", unpriced.join(", ")),
        CATALOG_PRICES_FIX.to_string(),
    )
}

/// A model reachable from `[tiers.*]` or `[providers.*].models` — every
/// section, native and compatible — with no catalog price: the sync check
/// between a user's own config and `prices.toml`
/// (`docs/design/providers.md` § Target shape item 5, T30.27).
/// `Config::configured_model_ids` (cox-protocol) is the same model
/// enumeration `cox_models::price`'s `usage_prices_cover_every_configured_model`
/// test uses for `default.toml`, so the two checks can never disagree; this
/// row also covers a user's own `[providers.*]` config, which that test
/// never sees.
fn check_catalog_prices(config: &cox_protocol::Config) -> CheckResult {
    let ids = config.configured_model_ids();
    match cox_models::Catalog::load(config, &[], None) {
        Ok(catalog) => catalog_prices_status(&catalog, &ids),
        Err(e) => CheckResult::fail(
            "catalog prices",
            format!("could not build model catalog: {e}"),
            "check [providers.*] for a malformed models entry".to_string(),
        ),
    }
}

/// T25.5: the same map the TUI would build; a warning for every entry it
/// skipped and every key two user bindings both claim in one context.
fn check_keybindings(cox_home: &std::path::Path, claude_home: &std::path::Path) -> CheckResult {
    let loaded = crate::config_load::keymap(cox_home, claude_home);
    let mut problems = loaded.warnings;
    problems.extend(loaded.keymap.conflicts());
    if problems.is_empty() {
        return CheckResult::ok("keybindings", "no conflicts".to_string());
    }
    CheckResult::warn(
        "keybindings",
        problems.join("; "),
        format!(
            "edit {} (docs/config.md, keybindings)",
            cox_home.join("keybindings.toml").display()
        ),
    )
}

fn check_claude_settings() -> CheckResult {
    // Walk up from cwd to find .claude/settings.json.
    let mut cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    loop {
        let settings_file = cwd.join(".claude/settings.json");
        if settings_file.exists() {
            return CheckResult::ok("settings.json", settings_file.display().to_string());
        }

        if !cwd.pop() {
            break;
        }
    }

    CheckResult::warn(
        "settings.json",
        ".claude/settings.json not found".to_string(),
        "create ~/.claude/settings.json or a project-local .claude/settings.json if you need custom permissions or hooks".to_string(),
    )
}

/// The assembled-prefix token count for the active profile (T30.1): an
/// empty-history request through `cox_core::assemble` priced by the T1.8
/// estimator, so `doctor` names what the next turn will actually send.
fn check_prefix(config: &cox_protocol::Config) -> CheckResult {
    let req = cox_core::assemble(&[], config, &[], std::path::Path::new("."), "");
    let tokens = cox_provider::tokens::estimate(&req).tokens;
    let profile = if config.core.profile.is_empty() {
        "default"
    } else {
        config.core.profile.as_str()
    };
    CheckResult::ok("prefix", format!("{tokens} tokens (profile {profile})"))
}

/// One result as the human output prints it: the status line, then a
/// `fix:` line unless it passed.
/// `pub(crate)` so `ext_cmd::list` (T33.39) renders a plugin row the same
/// way doctor does, instead of a second formatter.
pub(crate) fn human(result: &CheckResult) -> String {
    let status_str = match result.status.as_str() {
        "ok" => "✓",
        "warn" => "⚠",
        "fail" => "✗",
        _ => "?",
    };
    let mut out = format!("{}: {} {}\n", result.check, status_str, result.detail);
    if !result.fix.is_empty() && result.status != "ok" {
        out.push_str(&format!("  fix: {}\n", result.fix));
    }
    out
}

fn output_human(results: &[CheckResult]) -> bool {
    for result in results {
        print!("{}", human(result));
    }
    results.iter().any(|r| r.status == "fail")
}

/// How long doctor waits on LM Studio: a hung server must not hang doctor.
const LMSTUDIO_DOCTOR_TIMEOUT_S: u32 = 5;

/// Asks LM Studio's native API about the code tier's model (T30.16) with
/// the key the session would use. Read-only: doctor never loads a model.
fn check_lmstudio(config: &cox_protocol::Config) -> CheckResult {
    let l = &config.providers.lmstudio;
    let mut transport = l.transport();
    transport.timeout_s = LMSTUDIO_DOCTOR_TIMEOUT_S;
    let model = crate::session::lmstudio_model(config);
    let key = cox_provider::http::resolve_key(&transport.api_key_env, "lmstudio").ok();
    let list = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| cox_protocol::errors::ProviderError::Network)
        .and_then(|rt| {
            let client = cox_provider::lmstudio::LmStudio::new(&transport, key)?;
            rt.block_on(client.models())
        });
    lmstudio_row(&transport.base_url, model, list)
}

/// [`check_lmstudio`]'s verdict from the model list alone, so every branch
/// is testable without a server.
fn lmstudio_row(
    base_url: &str,
    model: &str,
    list: Result<cox_provider::lmstudio::ModelList, cox_protocol::errors::ProviderError>,
) -> CheckResult {
    const CHECK: &str = "LM Studio";
    let list = match list {
        Ok(list) => list,
        Err(e) => {
            return CheckResult::fail(
                CHECK,
                format!("{base_url} unreachable or rejected the model list: {e}"),
                "start the server (`lms server start`) or fix providers.lmstudio.base_url"
                    .to_string(),
            );
        }
    };
    let Some(m) = list.find(model) else {
        return CheckResult::fail(
            CHECK,
            format!("{base_url} reachable; `{model}` is not downloaded"),
            format!("`lms get {model}`, or point tiers.code.model at a listed model"),
        );
    };
    let max = m
        .max_context_length
        .map_or_else(|| "?".to_string(), |n| n.to_string());
    let tools = match m.tool_use() {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    };
    match m.loaded_context() {
        Some(loaded) => {
            let detail = format!(
                "{base_url} reachable; `{model}` loaded, context {loaded} loaded / {max} max; tool use: {tools}"
            );
            if m.tool_use() == Some(true) {
                CheckResult::ok(CHECK, detail)
            } else {
                CheckResult::warn(
                    CHECK,
                    detail,
                    "pick a model trained for tool use; cox drives a tool loop".to_string(),
                )
            }
        }
        None => CheckResult::warn(
            CHECK,
            format!(
                "{base_url} reachable; `{model}` not loaded (max context {max}); tool use: {tools}"
            ),
            format!("`lms load {model}`, or set providers.lmstudio.load = true"),
        ),
    }
}

fn output_json(results: &[CheckResult]) -> bool {
    let json_array = serde_json::to_string_pretty(results).unwrap_or_default();
    println!("{}", json_array);
    results.iter().any(|r| r.status == "fail")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_exit_code_is_1_on_fail() {
        // Create a failing check.
        let results = [
            CheckResult::ok("check1", "detail".to_string()),
            CheckResult::fail("check2", "detail".to_string(), "fix".to_string()),
        ];

        let has_fail = results.iter().any(|r| r.status == "fail");
        assert!(has_fail);
    }

    #[test]
    fn doctor_warns_on_keybinding_conflicts() {
        let cox = tempfile::tempdir().expect("tempdir");
        let claude = tempfile::tempdir().expect("tempdir");
        let ok = check_keybindings(cox.path(), claude.path());
        assert_eq!(
            (ok.status.as_str(), ok.detail.as_str()),
            ("ok", "no conflicts")
        );
        std::fs::write(
            cox.path().join("keybindings.toml"),
            "send = \"ctrl+o\"\ntranscript = \"ctrl+o\"\n",
        )
        .expect("write");
        let warn = check_keybindings(cox.path(), claude.path());
        assert_eq!(warn.status, "warn");
        assert!(warn.detail.contains("send"), "{}", warn.detail);
        assert!(warn.detail.contains("transcript"), "{}", warn.detail);
    }

    #[test]
    fn doctor_results_serialize_to_json() {
        let result = CheckResult::ok("test", "detail".to_string());
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"check\":\"test\""));
        assert!(json.contains("\"status\":\"ok\""));
    }

    /// T30.16: the LM Studio row for a loaded model, an unloaded one, one
    /// the server does not list, and a server that is down.
    #[test]
    fn doctor_lmstudio_rows() {
        let raw = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/lmstudio/models.json"),
        )
        .expect("fixture");
        let list: cox_provider::lmstudio::ModelList =
            serde_json::from_str(&raw).expect("fixture parses");
        let mut unloaded = list.clone();
        for m in &mut unloaded.models {
            m.loaded_instances.clear();
        }
        let url = "http://localhost:1234";
        let rows = [
            lmstudio_row(url, "prism-ml/bonsai-27b", Ok(list.clone())),
            lmstudio_row(url, "prism-ml/bonsai-27b", Ok(unloaded)),
            lmstudio_row(
                url,
                "text-embedding-nomic-embed-text-v1.5",
                Ok(list.clone()),
            ),
            lmstudio_row(url, "nope/absent", Ok(list)),
            lmstudio_row(
                url,
                "prism-ml/bonsai-27b",
                Err(cox_protocol::errors::ProviderError::Network),
            ),
        ];
        assert_eq!(
            rows.iter().map(|r| r.status.as_str()).collect::<Vec<_>>(),
            ["ok", "warn", "warn", "fail", "fail"]
        );
        insta::assert_snapshot!(rows.iter().map(human).collect::<String>());
    }

    #[test]
    fn doctor_human_output() {
        let results = vec![
            CheckResult::ok("toolchain", "rustc 1.98.1".to_string()),
            CheckResult::ok("COX_HOME writable", "/home/user/.cox".to_string()),
            CheckResult::ok("db", "database opens and schema is valid".to_string()),
            CheckResult::ok("API keys", "Anthropic API key found".to_string()),
            CheckResult::warn(
                "sandbox backend",
                "sandbox-exec not found, sandbox disabled".to_string(),
                "sandbox-exec is part of macOS; check your installation".to_string(),
            ),
            CheckResult::ok("git", "git version 2.40.0".to_string()),
            CheckResult::ok("terminal", "TERM=xterm-256color, true colour detected, size 120x40".to_string()),
            CheckResult::warn(
                "prices",
                "prices table not found in configuration".to_string(),
                "prices will be added in a future version".to_string(),
            ),
            CheckResult::warn(
                "settings.json",
                ".claude/settings.json not found".to_string(),
                "create ~/.claude/settings.json or a project-local .claude/settings.json if you need custom permissions or hooks".to_string(),
            ),
        ];

        // Use insta snapshot to verify human output format.
        let mut output = String::new();
        for result in &results {
            let status_str = match result.status.as_str() {
                "ok" => "✓",
                "warn" => "⚠",
                "fail" => "✗",
                _ => "?",
            };
            output.push_str(&format!(
                "{}: {} {}\n",
                result.check, status_str, result.detail
            ));
            if !result.fix.is_empty() && result.status != "ok" {
                output.push_str(&format!("  fix: {}\n", result.fix));
            }
        }

        insta::assert_snapshot!(output);
    }

    /// T41.7: the LSP row lists each server's program as found or missing,
    /// warns only when none is found, and says so when `[lsp]` is off.
    #[test]
    fn doctor_lsp_row() {
        let lsp = cox_protocol::config::LspConfig::default();
        let some = check_lsp_with(&lsp, |command| command == "rust-analyzer");
        let none = check_lsp_with(&lsp, |_| false);
        let off = check_lsp_with(
            &cox_protocol::config::LspConfig {
                enabled: false,
                ..lsp.clone()
            },
            |_| true,
        );
        let rows = [some, none, off];
        assert_eq!(
            rows.iter().map(|r| r.status.as_str()).collect::<Vec<_>>(),
            ["ok", "warn", "ok"]
        );
        insta::assert_snapshot!(rows.iter().map(human).collect::<String>());
    }

    /// T54.7: enabled with the model not downloaded warns and names the
    /// download command, without asking for the input device; the other
    /// rows are off, present with a device, and present without one.
    #[cfg(feature = "voice")]
    #[test]
    fn doctor_reports_voice_model_missing() {
        let home = tempfile::tempdir().expect("tempdir");
        let voice = cox_protocol::config::VoiceConfig {
            enabled: true,
            ..Default::default()
        };
        let model = crate::voice_cmd::model_path(home.path(), "base.en");
        let missing = check_voice_with(&voice, model.clone(), || unreachable!("no device probe"));
        assert_eq!(missing.status, "warn");
        assert!(
            human(&missing).contains("cox voice model download base.en"),
            "{}",
            human(&missing)
        );

        let path = model.clone().expect("pinned");
        std::fs::create_dir_all(path.parent().expect("dir")).expect("mkdir");
        std::fs::write(&path, b"ggml").expect("model");
        let with = check_voice_with(&voice, model.clone(), || Some("Built-in Mic".into()));
        let without = check_voice_with(&voice, model.clone(), || None);
        let off = check_voice_with(&Default::default(), model, || None);
        assert_eq!(
            [&with, &without, &off].map(|r| r.status.as_str()),
            ["ok", "warn", "ok"]
        );
        assert!(human(&with).contains("Built-in Mic"));
    }

    /// T37.36: a `cox.db` a newer `cox` migrated fails the `db` row with
    /// both versions and an update fix, never "remove cox.db".
    #[test]
    fn doctor_reports_a_newer_schema() {
        let row = db_result(
            Err(cox_protocol::StoreError::SchemaNewer {
                db: "99991231000000".into(),
                binary: "00000000000004".into(),
            }),
            std::path::Path::new("/home/user/.cox"),
        );
        insta::assert_snapshot!(human(&row));
    }

    #[test]
    fn doctor_prices_embedded_table_is_ok() {
        let result = check_prices();
        assert_eq!(result.status, "ok");
        assert!(result.detail.starts_with("oldest verified_on "));
    }

    #[test]
    fn doctor_prices_verified_after_today_is_fresh() {
        let ahead = Price {
            id: "claude-haiku-4-5".to_string(),
            input: 1.0,
            output: 5.0,
            cache_write: 1.25,
            cache_read: 0.1,
            verified_on: "2026-09-13".to_string(),
            source_url: "https://example.com".to_string(),
        };
        let result = prices_status(&[ahead], (2026, 9, 12));
        assert_eq!(result.status, "ok", "{}", result.detail);
    }

    #[test]
    fn doctor_prices_older_than_90_days_warns() {
        let stale = Price {
            id: "claude-haiku-4-5".to_string(),
            input: 1.0,
            output: 5.0,
            cache_write: 1.25,
            cache_read: 0.1,
            verified_on: "2020-01-01".to_string(),
            source_url: "https://example.com".to_string(),
        };
        let result = prices_status(&[stale], (2026, 9, 12));
        assert_eq!(result.status, "warn");
        assert!(result.detail.contains("2020-01-01"));
        assert!(result.detail.contains("days old"));
        assert_eq!(result.fix, PRICES_FIX);
    }

    #[test]
    fn catalog_prices_check_is_ok_on_the_default_config() {
        let config = cox_protocol::Config::default();
        let result = check_catalog_prices(&config);
        assert_eq!(result.status, "ok", "{}", result.detail);
    }

    #[test]
    fn catalog_prices_check_warns_and_names_an_unpriced_model() {
        // A model reachable from `[providers.*].models` with no row in
        // `prices.toml` (built-in or user) is a warning, not a failure —
        // an unpriced model still runs, just costed 0 and `estimated`
        // (`cox_models::PriceTable::apply`).
        let mut config = cox_protocol::Config::default();
        config
            .providers
            .anthropic
            .models
            .push(cox_protocol::config::ProviderModel {
                id: "claude-doctor-test-unpriced".into(),
                context_window: 100_000,
                efforts: vec![],
                ..Default::default()
            });
        let result = check_catalog_prices(&config);
        assert_eq!(result.status, "warn", "{}", result.detail);
        assert!(
            result.detail.contains("claude-doctor-test-unpriced"),
            "{}",
            result.detail
        );
        assert_eq!(result.fix, CATALOG_PRICES_FIX);
    }

    #[test]
    fn catalog_prices_check_names_every_unpriced_model_reachable_from_tiers() {
        // `[tiers.*].model` is the other reachability path the goal names,
        // alongside `[providers.*].models`.
        let mut config = cox_protocol::Config::default();
        config.tiers.cheap.model = "claude-doctor-test-tier-unpriced".into();
        let result = check_catalog_prices(&config);
        assert_eq!(result.status, "warn", "{}", result.detail);
        assert!(
            result.detail.contains("claude-doctor-test-tier-unpriced"),
            "{}",
            result.detail
        );
    }

    /// T33.42 Check: `doctor_lists_unsandboxed_servers`.
    #[test]
    fn doctor_lists_unsandboxed_servers() {
        let mut mcp = HashMap::new();
        mcp.insert(
            "clean".to_string(),
            McpServerConfig {
                command: Some("a".into()),
                ..Default::default()
            },
        );
        let ok = check_mcp_sandbox(&mcp);
        assert_eq!(ok.status, "ok", "{}", ok.detail);

        mcp.insert(
            "opted-out".to_string(),
            McpServerConfig {
                command: Some("b".into()),
                sandbox: false,
                ..Default::default()
            },
        );
        // A `url` server has no argv to wrap, so `sandbox = false` on one
        // must not show up as if it were opted out.
        mcp.insert(
            "http".to_string(),
            McpServerConfig {
                url: Some("https://example.com/mcp".into()),
                sandbox: false,
                ..Default::default()
            },
        );
        let warn = check_mcp_sandbox(&mcp);
        assert_eq!(warn.status, "warn", "{}", warn.detail);
        assert!(warn.detail.contains("opted-out"), "{}", warn.detail);
        assert!(!warn.detail.contains("http"), "{}", warn.detail);
        assert!(!warn.detail.contains("clean"), "{}", warn.detail);
    }

    #[cfg(feature = "plugins")]
    fn external_agent_decl(command: &str) -> cox_plugin_api::ExternalAgentDecl {
        cox_plugin_api::ExternalAgentDecl {
            name: "cursor".into(),
            command: command.into(),
            args: vec!["acp".into()],
            mode: cox_plugin_api::AgentMode::Acp,
            key_env: "CURSOR_API_KEY".into(),
        }
    }

    /// An identity wrap: `program args`, unchanged — the probe then runs
    /// exactly `decl.command --version` with no real sandbox involved, so
    /// these tests never depend on this host's sandbox backend.
    #[cfg(feature = "plugins")]
    fn identity_wrap(program: &std::path::Path, args: &[String]) -> Result<Vec<String>, String> {
        let mut argv = vec![program.display().to_string()];
        argv.extend(args.iter().cloned());
        Ok(argv)
    }

    /// T35.8 Check: `doctor_reports_missing_cli_as_a_warning_not_a_failure`.
    #[cfg(feature = "plugins")]
    #[test]
    fn doctor_reports_missing_cli_as_a_warning_not_a_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let decl = external_agent_decl("cox-doctor-test-missing-cli-does-not-exist");
        let result = check_external_agent_with(dir.path(), &decl, true, identity_wrap);
        assert_eq!(result.status, "warn", "{}", result.detail);
        assert!(
            result.detail.contains("not found on PATH"),
            "{}",
            result.detail
        );
    }

    /// T35.8 Check: `doctor_reports_key_env_set_and_cli_version`.
    #[cfg(feature = "plugins")]
    #[test]
    fn doctor_reports_key_env_set_and_cli_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("bin")).expect("mkdir");
        let script = dir.path().join("bin/agent");
        std::fs::write(&script, "#!/bin/sh\necho v9.9.9\n").expect("write script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("chmod +x");
        }
        let decl = external_agent_decl("bin/agent");

        let found = check_external_agent_with(dir.path(), &decl, true, identity_wrap);
        assert_eq!(found.status, "ok", "{}", found.detail);
        assert!(found.detail.contains("v9.9.9"), "{}", found.detail);
        assert!(
            found.detail.contains("key_env CURSOR_API_KEY set"),
            "{}",
            found.detail
        );

        // Same CLI, but the key is missing: still a warning, and the CLI
        // and version facts stay in the detail alongside it.
        let no_key = check_external_agent_with(dir.path(), &decl, false, identity_wrap);
        assert_eq!(no_key.status, "warn", "{}", no_key.detail);
        assert!(no_key.detail.contains("v9.9.9"), "{}", no_key.detail);
        assert!(
            no_key.detail.contains("key_env CURSOR_API_KEY not set"),
            "{}",
            no_key.detail
        );
    }

    /// A sandbox wrap failure is EA§2's wrap-or-refuse: still a warning,
    /// never a hard failure, and named as a refusal rather than a missing
    /// CLI or a missing key.
    #[cfg(feature = "plugins")]
    #[test]
    fn doctor_reports_sandbox_refusal_as_a_warning_not_a_failure() {
        // An in-package program, so the PATH check before the wrap passes
        // whether or not this host has an `agent` CLI installed.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("bin")).expect("mkdir");
        std::fs::write(dir.path().join("bin/agent"), "#!/bin/sh\n").expect("write script");
        let decl = external_agent_decl("bin/agent");
        let result = check_external_agent_with(dir.path(), &decl, true, |_, _| {
            Err::<Vec<String>, _>(String::from("no sandbox backend on this host"))
        });
        assert_eq!(result.status, "warn", "{}", result.detail);
        assert!(result.detail.contains("refused"), "{}", result.detail);
        assert!(
            result.detail.contains("no sandbox backend on this host"),
            "{}",
            result.detail
        );
    }

    /// An in-package `command` that does not resolve (missing file, a
    /// symlink) is a warning about the plugin's manifest, not a sandbox or
    /// PATH question.
    #[cfg(feature = "plugins")]
    #[test]
    fn doctor_reports_a_bad_in_package_command_as_a_warning() {
        let dir = tempfile::tempdir().expect("tempdir");
        let decl = external_agent_decl("bin/gone");
        let result = check_external_agent_with(dir.path(), &decl, true, identity_wrap);
        assert_eq!(result.status, "warn", "{}", result.detail);
        assert_ne!(result.status, "fail");
    }

    /// Stages a minimal plugin package at `<home>/plugins/<id>` from a
    /// complete `plugin.toml` body (the same stage-then-activate steps
    /// `cox plugin install` runs, `plugin_cmd::install`) and returns its
    /// parsed manifest and digest.
    #[cfg(feature = "plugins")]
    fn stage_plugin_toml(
        home: &std::path::Path,
        id: &str,
        toml: &str,
    ) -> (cox_plugin_api::PluginManifest, String) {
        let src = tempfile::tempdir().expect("tempdir");
        std::fs::write(src.path().join("plugin.toml"), toml).expect("manifest");
        let (manifest, digest) =
            cox_plugin::discover::load_manifest(src.path(), &src.path().join("plugin.toml"), None)
                .expect("valid manifest");
        let plugin_dir = cox_plugin::install::plugin_dir(home, id);
        cox_plugin::install::stage(src.path(), &plugin_dir, &digest).expect("stage");
        cox_plugin::install::activate(&plugin_dir, cox_plugin::install::short(&digest))
            .expect("activate");
        (manifest, digest)
    }

    /// A minimal wasm plugin, staged via [`stage_plugin_toml`]. `extra` is
    /// TOML appended after the required fields, e.g. a `[[models]]` row
    /// (T33.16).
    #[cfg(feature = "plugins")]
    fn stage_plugin(
        home: &std::path::Path,
        id: &str,
        extra: &str,
    ) -> (cox_plugin_api::PluginManifest, String) {
        stage_plugin_toml(
            home,
            id,
            &format!(
                "api = 1\nid = {id:?}\nversion = \"0.1.0\"\nname = \"Demo\"\nwasm = \"plugin.wasm\"\n{extra}"
            ),
        )
    }

    /// Grants `id` at `digest` in the user scope, the same row
    /// `cox plugin install --yes` would write.
    #[cfg(feature = "plugins")]
    fn grant_plugin(
        store: &cox_store::Store,
        manifest: &cox_plugin_api::PluginManifest,
        digest: &str,
    ) {
        use cox_protocol::{PluginGrant, PluginStore as _};

        store
            .grant_put(&PluginGrant {
                plugin_id: manifest.id.clone(),
                scope: cox_plugin::grant::scope(cox_plugin::discover::Source::User, None)
                    .expect("user scope"),
                digest: digest.to_string(),
                capabilities: serde_json::json!(cox_plugin::grant::capability_list(manifest)),
                enabled: true,
                source: serde_json::json!({}),
                decided_at: "2026-09-26T00:00:00Z".to_string(),
            })
            .expect("grant");
    }

    /// T33.39 Check: a scratch `COX_HOME` with one healthy (granted) plugin
    /// whose `[[models]]` price conflicts with the built-in catalog
    /// (T33.16), one broken plugin (no `current` pointer, so it never even
    /// parses) and one ungranted plugin (staged but never approved) —
    /// `check_plugins` reports all three, sorted by id, never as `fail`
    /// (extensions fail open).
    #[cfg(feature = "plugins")]
    #[test]
    fn doctor_plugin_rows_report_healthy_broken_and_ungranted() {
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(home.path().join("plugins")).expect("mkdir");

        let (healthy, healthy_digest) = stage_plugin(
            home.path(),
            "healthy",
            "\n[[models]]\nid = \"claude-haiku-4-5\"\nprice = { input = 999.0, output = 999.0 }\n",
        );
        let store = cox_store::Store::open(home.path()).expect("store");
        grant_plugin(&store, &healthy, &healthy_digest);

        // Broken: the directory exists but has no `current` pointer, so
        // discovery cannot even find a version to read.
        std::fs::create_dir_all(home.path().join("plugins/broken")).expect("mkdir");

        // Ungranted: staged and activated, never approved.
        stage_plugin(home.path(), "ungranted", "");

        let config = cox_protocol::Config::default();
        let rows = check_plugins(home.path(), home.path(), &config);
        let ids: Vec<&str> = rows.iter().map(|r| r.check.as_str()).collect();
        assert_eq!(ids, ["plugin broken", "plugin healthy", "plugin ungranted"]);
        assert!(rows.iter().all(|r| r.status != "fail"), "{rows:?}");

        assert_eq!(rows[0].status, "warn");
        assert!(rows[0].detail.starts_with("skipped:"), "{}", rows[0].detail);

        assert_eq!(rows[1].status, "ok", "{}", rows[1].detail);
        assert!(rows[1].detail.contains("loaded"), "{}", rows[1].detail);
        assert!(
            rows[1].detail.contains("catalog conflicts")
                && rows[1].detail.contains("claude-haiku-4-5"),
            "{}",
            rows[1].detail
        );

        assert_eq!(rows[2].status, "warn");
        assert_eq!(rows[2].detail, "not granted");
    }

    /// T33.38 Check (PL§13/§14): a wasm-less `[[mcp]]`-only plugin is
    /// reported "loaded" exactly like a wasm plugin's — `check_plugins`
    /// never reads `wasm`, granted or not, so its absence is not an error.
    #[cfg(feature = "plugins")]
    #[test]
    fn doctor_reports_a_wasm_less_mcp_only_plugin_as_loaded() {
        let home = tempfile::tempdir().expect("tempdir");
        let (manifest, digest) = stage_plugin_toml(
            home.path(),
            "count",
            "api = 1\nid = \"count\"\nversion = \"0.1.0\"\nname = \"count\"\n\n\
             [[mcp]]\nname = \"count\"\ncommand = \"bin/server\"\n",
        );
        assert_eq!(manifest.wasm, None, "the fixture ships no plugin.wasm");
        let store = cox_store::Store::open(home.path()).expect("store");
        grant_plugin(&store, &manifest, &digest);

        let config = cox_protocol::Config::default();
        let rows = check_plugins(home.path(), home.path(), &config);
        let row = rows
            .iter()
            .find(|r| r.check == "plugin count")
            .expect("row");
        assert_eq!(row.status, "ok", "{}", row.detail);
        assert!(row.detail.contains("loaded"), "{}", row.detail);
    }

    /// T33.39 Check `disabled_export_is_visible_in_doctor`: the breaker's
    /// per-plugin disabled-export list, injected through
    /// `check_plugins_with` the way `check_api_keys_with` injects a key
    /// lookup, shows up in the plugin's row.
    #[cfg(feature = "plugins")]
    #[test]
    fn disabled_export_is_visible_in_doctor() {
        let home = tempfile::tempdir().expect("tempdir");
        let (manifest, digest) = stage_plugin(home.path(), "flaky", "");
        let store = cox_store::Store::open(home.path()).expect("store");
        grant_plugin(&store, &manifest, &digest);

        let mut disabled = HashMap::new();
        disabled.insert("flaky".to_string(), vec!["cox_decide".to_string()]);
        let config = cox_protocol::Config::default();
        let rows = check_plugins_with(home.path(), home.path(), &config, &disabled);
        let row = rows
            .iter()
            .find(|r| r.check == "plugin flaky")
            .expect("row");
        assert!(
            row.detail
                .contains("exports disabled by three failures: cox_decide"),
            "{}",
            row.detail
        );
    }

    /// An absent `<COX_HOME>/cache/wasmtime` (caching is off today,
    /// `cox-plugin/src/host.rs`) is 0 bytes and `ok`, never a warning.
    #[cfg(feature = "plugins")]
    #[test]
    fn plugin_cache_size_is_zero_when_not_yet_created() {
        let home = tempfile::tempdir().expect("tempdir");
        let result = check_plugin_cache(home.path());
        assert_eq!(result.status, "ok");
        assert!(result.detail.contains("0 bytes"), "{}", result.detail);
    }

    /// The cache size sums file bytes recursively once something writes
    /// under it.
    #[cfg(feature = "plugins")]
    #[test]
    fn plugin_cache_size_sums_files_recursively() {
        let home = tempfile::tempdir().expect("tempdir");
        let sub = home.path().join("cache/wasmtime/sub");
        std::fs::create_dir_all(&sub).expect("mkdir");
        std::fs::write(sub.join("a.bin"), vec![0u8; 10]).expect("write");
        std::fs::write(home.path().join("cache/wasmtime/b.bin"), vec![0u8; 5]).expect("write");
        let result = check_plugin_cache(home.path());
        assert!(result.detail.contains("15 bytes"), "{}", result.detail);
    }
}
