//! `cox-plugin-fixtures`: plugin packages built from source for cox's own
//! end-to-end tests and `just bench` (PL§13). A crate of its own so only
//! the crates that dev-depend on it pay for the `wasm32-unknown-unknown`
//! build its `build.rs` runs; no `.wasm` is committed, so nothing drifts
//! from `plugins/examples/rust` (A48).

// why: this crate was not under `missing_docs` before the workspace lints;
// drop this allow once its public items are documented.
#![allow(missing_docs)]

/// The Rust reference example as an installable package directory: its
/// `plugin.toml` and `example.wasm`, nothing else, so `cox plugin install`
/// digests exactly what a user's build would ship.
pub const EXAMPLE_DIR: &str = concat!(env!("OUT_DIR"), "/example");

/// The example's module bytes, for a host test that loads it directly.
pub const EXAMPLE_WASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/example/example.wasm"));

/// The example's manifest text.
pub const EXAMPLE_MANIFEST: &str = include_str!(concat!(env!("OUT_DIR"), "/example/plugin.toml"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_package_holds_its_manifest_and_a_wasm_module() {
        assert!(EXAMPLE_WASM.starts_with(b"\0asm"), "not a wasm module");
        assert!(EXAMPLE_MANIFEST.contains("wasm = \"example.wasm\""));
        let mut files: Vec<_> = std::fs::read_dir(EXAMPLE_DIR)
            .expect("package dir")
            .map(|e| e.expect("entry").file_name())
            .collect();
        files.sort();
        assert_eq!(files, ["example.wasm", "plugin.toml"]);
    }
}
