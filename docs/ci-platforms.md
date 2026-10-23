# CI platforms

Principle: everything that can be built, checked or tested on Linux runs on Linux
(`ubuntu-latest` / `ubuntu-24.04`, plus `ubuntu-24.04-arm` for the aarch64 target). Windows and
macOS runners keep only what exists or matters only on that platform. Those jobs are optional
(`continue-on-error`), so they never fail the `CI` workflow. No platform test run was removed:
the macOS Rust tests moved from the `rust` matrix to their own job.

The shared checks come from [pyrlyn/ci](https://github.com/pyrlyn/ci) through
`.github/workflows/pipeline.yml`.

## Gate

`pipeline / gate` is pyrlyn/ci's gate job and is meant to be the only required check. It covers
pyrlyn/ci's own jobs (config, CodeQL, Semgrep, actionlint, license, commits, no-pinned-ci, ...),
not this repository's `ci.yml` jobs. It fails only when a needed job's result is `failure`:
`skipped` and `cancelled` pass. There is no local gate job.

Until the `protect-main` ruleset is reduced to `pipeline / gate`, it still lists the Linux
`ci.yml` jobs and `footprint (xcode-27)`, which is now optional.

## Per job

| Job | Platform | Removed from Windows/macOS | Kept there, and why |
|-----|----------|----------------------------|---------------------|
| `rust / fmt`, `rust / x86_64-unknown-linux-gnu`, `rust / aarch64-unknown-linux-gnu` (clippy, check, full nextest suite) | Linux | the `aarch64-apple-darwin` matrix entry (full suite, clippy, check) | nothing |
| `macos` | macOS, optional | - | the `cfg(target_os = "macos")` Rust tests (9: Seatbelt sandbox and what runs under it), picked by `PLATFORM_TESTS` |
| `windows` | Windows, disabled (`if: false`) | - | `just windows-check`; off until the P57 Windows work is built on a Windows host (see Known gaps) |
| `desktop-macos`, `desktop-macos-lint` | macOS, optional | the three Python script unit tests (now `scripts`) | `swift test`, the app build, SwiftLint, swift-format, brand icons (resvg output is pinned to Apple Silicon) |
| `desktop-windows` | Windows, optional | - | the WinUI app build and its MSTest |
| `footprint (xcode-27)` | macOS, optional | - | the macOS footprint baseline (keyed per OS-arch) |
| `scripts` | Linux | - | new: `test_swift_test_select.py`, `test_sccache_config.py`, `test_xcframework.py` (fakes only, no Xcode) |
| `sandbox-landlock`, `plugins`, `plugin-examples`, `deny`, `hakari`, `audit`, `footprint (ubuntu-24.04)`, `desktop-tokens`, `docs`, `config`, `changes`, `macos-runner` | Linux | none (never ran elsewhere) | nothing |
| `pipeline / *` | Linux (pyrlyn/ci defaults) | - | nothing |

The Swift code is macOS-only by nature (AppKit/SwiftUI, Xcode 27 SDK, snapshot tests per
toolchain), so the Swift jobs stay on macOS whole; the scheme for them is "the job runs only
where it can run", not a test filter.

## Keeping the platform filters current

`macos` lists its tests in `PLATFORM_TESTS` (`.github/workflows/ci.yml`) as a nextest filterset,
`test(/(^|::)<name>$/)` per test. A new or renamed test gated to macOS must be added there, or it
runs nowhere. An empty selection passes (`--no-tests=pass`). Tests gated to `unix` or `linux` run
on Linux and are not listed.

## Known gaps

- Windows: 7 `cfg(windows)` Rust tests exist (cox-sandbox path confinement, cox-tools,
  cox-app, cox) but run nowhere, because the workspace does not build on Windows yet (P57) and
  the `windows` job stays disabled by the creator's decision. When it is enabled, it should run
  them through a `PLATFORM_TESTS` filter the same way.
- Clippy and check run only for Linux targets, so `cfg(target_os = "macos")` code is compiled by
  the optional `macos` job but no longer linted in CI.
- The `protect-main` ruleset change above is a repository setting and is not part of this PR.
