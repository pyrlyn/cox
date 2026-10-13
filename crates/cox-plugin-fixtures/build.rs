//! Builds `plugins/examples/rust` and `plugins/examples/surfaces` for
//! `wasm32-unknown-unknown` with the guest workspace's own cargo and lays
//! each out as an installable package in `OUT_DIR/example` and
//! `OUT_DIR/surfaces` (PL§13, T33.45.8). A nested cargo, not a dependency, because a
//! dependency would build for the host target of the main workspace.
//!
//! The nested build gets its own `--target-dir` under `OUT_DIR`, so it never
//! waits on the outer build's lock, and it shares the outer build's job
//! slots through the inherited jobserver (`CARGO_MAKEFLAGS`).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const TARGET: &str = "wasm32-unknown-unknown";

/// One package to build: the guest crate, its directory under `plugins/`,
/// the cargo artifact, and the package folder and `.wasm` name its
/// `plugin.toml` expects.
struct Fixture {
    krate: &'static str,
    dir: &'static str,
    artifact: &'static str,
    package: &'static str,
    wasm: &'static str,
}

const FIXTURES: [Fixture; 2] = [
    Fixture {
        krate: "cox-plugin-example",
        dir: "examples/rust",
        artifact: "cox_plugin_example.wasm",
        package: "example",
        wasm: "example.wasm",
    },
    Fixture {
        krate: "cox-plugin-surfaces",
        dir: "examples/surfaces",
        artifact: "cox_plugin_surfaces.wasm",
        package: "surfaces",
        wasm: "surfaces.wasm",
    },
];

fn main() {
    if let Err(e) = build() {
        // `cargo::error` fails the build with this line as the reason.
        println!("cargo::error={e}");
    }
}

fn var(name: &str) -> Result<String, String> {
    env::var(name).map_err(|e| format!("{name}: {e}"))
}

fn build() -> Result<(), String> {
    let manifest_dir = PathBuf::from(var("CARGO_MANIFEST_DIR")?);
    let root = manifest_dir.join("../..");
    let plugins = root.join("plugins");
    let watched = [
        plugins.join("Cargo.toml"),
        plugins.join("Cargo.lock"),
        plugins.join("sdk"),
        root.join("crates/cox-plugin-api"),
    ];
    let sources = FIXTURES.iter().map(|f| plugins.join(f.dir));
    for path in watched.into_iter().chain(sources) {
        println!("cargo::rerun-if-changed={}", path.display());
    }
    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    require_target(&rustc)?;

    let out = PathBuf::from(var("OUT_DIR")?);
    let target_dir = out.join("wasm-target");
    // One nested cargo for every fixture: the shared dependencies (the SDK,
    // extism-pdk, serde) build once.
    let mut cargo = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cargo
        .current_dir(&plugins)
        .args(["build", "--release", "--locked", "--target", TARGET]);
    for fixture in &FIXTURES {
        cargo.args(["-p", fixture.krate]);
    }
    let status = cargo
        .arg("--target-dir")
        .arg(&target_dir)
        .env("CARGO_INCREMENTAL", "0")
        // Flags and wrappers the outer build set for the host target (or for
        // clippy on the main workspace) must not reach the guest build.
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("CLIPPY_ARGS")
        .status()
        .map_err(|e| format!("cannot run cargo for the plugin fixtures: {e}"))?;
    if !status.success() {
        return Err(format!(
            "building the plugin fixtures for {TARGET} failed ({status})"
        ));
    }

    let release = target_dir.join(TARGET).join("release");
    for fixture in &FIXTURES {
        let package = out.join(fixture.package);
        fs::create_dir_all(&package).map_err(|e| format!("{}: {e}", package.display()))?;
        copy(&release.join(fixture.artifact), &package.join(fixture.wasm))?;
        copy(
            &plugins.join(fixture.dir).join("plugin.toml"),
            &package.join("plugin.toml"),
        )?;
    }
    // ~130 MB of guest build output would otherwise stay in every OUT_DIR
    // (the disk is shared by parallel worktrees). Cargo reruns this script
    // only when a watched source changes, so the price is one full guest
    // build per such change.
    fs::remove_dir_all(&target_dir).map_err(|e| format!("{}: {e}", target_dir.display()))
}

/// Fails with the fix when the toolchain lacks the wasm32 standard library,
/// instead of letting the nested build end in "can't find crate for `core`".
fn require_target(rustc: &str) -> Result<(), String> {
    let out = Command::new(rustc)
        .args(["--print", "target-libdir", "--target", TARGET])
        .output()
        .map_err(|e| format!("cannot run {rustc}: {e}"))?;
    let libdir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() && Path::new(&libdir).is_dir() {
        return Ok(());
    }
    Err(format!(
        "the {TARGET} target is not installed for {rustc}; run `mise install` in the \
         repository root (mise.toml pins rust with that target) to build the plugin fixtures"
    ))
}

fn copy(from: &Path, to: &Path) -> Result<(), String> {
    fs::copy(from, to)
        .map(drop)
        .map_err(|e| format!("copy {} -> {}: {e}", from.display(), to.display()))
}
