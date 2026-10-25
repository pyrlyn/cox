# Dist profile: light optimization, no LTO

## How it works
The `dist` profile (used by cargo-dist and scripts/package.sh, the release CLI archives) now inherits `release`:
`opt-level = 1`, `lto = "off"`, `codegen-units = 256`, `incremental = true`,
`debug = "line-tables-only"`, `strip = "debuginfo"`. It builds close to dev speed but
produces a much smaller and faster binary than the former `inherits = "dev"`.
`panic` stays `unwind` on purpose: the code relies on `catch_unwind` / `JoinError` to
fail open, which `panic = "abort"` would break.

## Enable
`cargo build --profile dist` (cargo-dist uses the `dist` profile by default).

## Disable
Set `[profile.dist] inherits = "dev"` again (and drop the other keys).

## Test
`cargo build --profile dist` must succeed; the binary is in `target/dist/`.
Release CI builds through cargo-dist with the same profile.

## Limitations
- Not a full release build: no LTO, opt-level 1. For the smallest/fastest binary use
  `lto = "thin"` or `"fat"` with fewer codegen units, at a much longer link.
- `strip = "debuginfo"` keeps symbol names, so backtraces show function names but not
  `file:line` (the line tables are stripped from the shipped binary).
- `incremental = true` helps local rebuilds; CI from a clean cache does not benefit.
