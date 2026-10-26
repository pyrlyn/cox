# Dev profile: split debug info and optimized build scripts

## How it works
- `[profile.dev] split-debuginfo = "unpacked"` leaves debug info in the object files
  instead of running `dsymutil` to pack a `.dSYM` on every macOS link. Linux already
  defaults to this; Windows (MSVC) always writes a `.pdb` and ignores the key.
- `[profile.dev.build-override] opt-level = 3, debug = false` compiles build scripts and
  proc macros (serde, clap, tokio, …) optimized and without debug info. They run on every
  build that expands a derive, so faster macros mean faster incremental builds.

## Enable
It is on for `cargo build`, `cargo test`, `cargo run` and any profile that inherits `dev`.

## Disable
Remove the two keys from `Cargo.toml`, or override per run:
`CARGO_PROFILE_DEV_SPLIT_DEBUGINFO=packed` and `CARGO_PROFILE_DEV_BUILD_OVERRIDE_OPT_LEVEL=0`.

## Test
`cargo build` must succeed; on macOS no `.dSYM` dirs appear next to the binaries in
`target/debug/`. Backtraces and `lldb` still resolve `file:line` from the object files.

## Limitations
- Debug info now lives in `target/debug/deps/*.o`; copying only the binary to another
  machine loses symbols (use the dist/release profile for that).
- A cold build pays a little extra once to optimize build scripts and proc macros.
- A build script that relies on debug assertions no longer has them.
