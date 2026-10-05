# Toolchain

Programs the project uses and the direct packages from its manifests.

## Programs

| Program | How to install | Why here | Source |
| --- | --- | --- | --- |
| mise | brew / curl, then `mise install` | Pins tool versions | https://github.com/jdx/mise |
| cargo-cache | mise | `just cache` / `just cache-autoclean`; the shared cargo home fills up | https://github.com/matthiaskrgr/cargo-cache |
| rust | mise (with the `wasm32-unknown-unknown` target) | Compiler and std; the target builds the guest workspace `plugins/` (T33.27) and, through `cox-plugin-fixtures`' `build.rs`, the example plugin every `cargo nextest run --workspace` needs (T33.28) | https://github.com/rust-lang/rust |
| rustc | mise (pin rust) | Rust compiler | https://github.com/rust-lang/rust |
| cargo | mise (pin rust) | Rust builds and dependencies | https://github.com/rust-lang/cargo |
| cargo-nextest | global (cargo install / brew) | Parallel test runner | https://github.com/nextest-rs/nextest |
| just | cargo install just / brew | Command recipes | https://github.com/casey/just |
| ketch | see its README | Installs swarfr | https://github.com/pyrlyn/ketch |
| swarfr | ketch | `just check-all` ends with a lossless cleanup of `target/` | https://github.com/listepo/swarfr |
| uv | global (curl installer / brew) | Runs the `evals/` and `scripts/vendor/` packages (`just eval`, `just vendor`) and locks their Python deps | https://github.com/astral-sh/uv |
| python | uv (`evals/.python-version`, `scripts/vendor/.python-version`) | Eval harness, the Terminal-Bench agent (must be a Python class), `cox-vendor` (T30.19: vendored files no package manager fetches), and `scripts/changed_tests.py`, which picks the crates `just test` runs (T50.7; stdlib only) | https://github.com/python/cpython |
| zig | mise (`mise.toml`) | Linker for `cargo zigbuild`: the Linux cox the Terminal-Bench containers run (T30.9) | https://github.com/ziglang/zig |
| cargo-zigbuild | mise (`mise.toml`, aqua) | Cross-builds that Linux cox from macOS without a Docker build step | https://github.com/rust-cross/cargo-zigbuild |
| cargo-xwin | mise (`mise.toml`, github; macOS and Linux only) | `just windows-check` off Windows (T57.1): `cargo check` and clippy for `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc` through its `clang` backend, whose Windows sysroot it fetches into its cache; the recipe adds both targets to the pinned Rust with `rustup target add` | https://github.com/rust-cross/cargo-xwin |
| colima | global (mise) | Docker runtime for Terminal-Bench (the creator's choice) | https://github.com/abiosoft/colima |
| LM Studio (`lms`) | desktop app | Local model server for the eval matrix (`cox-bench`, provider `lmstudio`): OpenAI Chat and Anthropic Messages endpoints on :1234 | https://lmstudio.ai/docs/developer |
| docker-cli, docker-compose, docker-buildx | global (mise) | Harbor drives task containers through `docker compose` and `docker buildx build` | https://github.com/docker/cli , https://github.com/docker/compose , https://github.com/docker/buildx |
| dart | mise (`plugins/mise.toml`) | Builds `plugins/templates/dart` and `plugins/examples/dart` (T33.38): Dart cannot emit a wasm module extism can load (research.md §4.3.5 P44), so its only plugin capability is an `[[mcp]]` stdio server, `dart compile exe` | https://github.com/dart-lang/sdk |
| node | mise (`mise.toml`) | Runs Style Dictionary, the desktop token build (`just desktop-tokens`, T37.17, DS§2) | https://github.com/nodejs/node |
| SwiftLint | mise (`mise.toml`, aqua) | Lints the macOS app (T37.18): default rules plus DS§9's no-literal custom rules in `desktop/macos/.swiftlint.yml`; CI's `desktop-macos-lint` job; packages run the same version through the SwiftLintPlugins build-tool plugin | https://github.com/realm/SwiftLint |
| swift-format | Xcode toolchain (`xcrun swift-format`) | Formats the macOS app's Swift (research.md §9.5.5); CI's `desktop-macos-lint` job runs `lint --strict` | https://github.com/swiftlang/swift-format |
| XcodeGen | mise (`mise.toml`, aqua) | Generates the macOS app's thin `desktop/macos/Cox.xcodeproj` from `desktop/macos/project.yml`, so only the spec is in git and the project never merge-conflicts (`just desktop-app`, T37.32.1); CI's `desktop-macos` job | https://github.com/yonaskolb/XcodeGen |
| cmake | mise (`mise.toml`) | Builds whisper.cpp for `cox-voice` through `whisper-rs-sys` (T54.2, A123); 3.31.12, the version sibling projects already pin (apps/bindsmith) | https://github.com/Kitware/CMake |
| GNU gettext (`msgfmt`, `msgmerge`, `msginit`, `msgcmp`) | brew (`brew install gettext`); optional | Maintains and validates the `cox-i18n` catalogs: `just i18n-update` (msgmerge the template into each `.po`), `just i18n-check` (msgfmt --check, msgcmp), `msginit` for a new locale (`docs/i18n.md`). The runtime does not use it; the `cox-i18n` test that runs msgfmt skips when it is absent | https://www.gnu.org/software/gettext/ |

## ketch

| Package | Where | Source | Why here |
| --- | --- | --- | --- |
| swarfr | global | https://github.com/listepo/swarfr | Lossless `target/` cleanup after tests |

## cargo

| Package | Where | Source | Why here |
| --- | --- | --- | --- |
| agent-client-protocol | local | https://crates.io/crates/agent-client-protocol | cox-acp; crates/cox (the ACP external-agent driver, T35.13) |
| anyhow | local | https://crates.io/crates/anyhow | CLI errors |
| arboard | local | https://crates.io/crates/arboard | Rust dependency |
| assert_cmd | local | https://crates.io/crates/assert_cmd | Rust dependency |
| assert_fs | local | https://crates.io/crates/assert_fs | Rust dependency |
| async-openai | local (`response-types` only) | https://github.com/64bit/async-openai | cox-provider: OpenAI Responses request and stream event types (T30.11); transport stays ours |
| async-trait | local | https://crates.io/crates/async-trait | Rust dependency |
| base64 | local | https://github.com/marshallpierce/rust-base64 | cox-protocol: image attachments and tool-output payloads (T40.1); cox-tui: the OSC 52 clipboard payload (A81); cox-core: decodes an attached text file (`UserTurn.attachments`, T37.6) |
| uniffi | local | https://github.com/mozilla/uniffi-rs | cox-ffi: Swift bindings for the macOS app (T37.14), proc-macros, no UDL |
| syn | local | https://github.com/dtolnay/syn | cox-ffi dev-dependency: `tests/forward_only.rs` parses the FFI sources to enforce D11's forward-only rule (T37.39.1, A90) |
| bytes | local | https://crates.io/crates/bytes | T1.2: turns a reqwest byte stream into SSE frames (`sse.rs`) and drives the in-memory fixture parser (`parse_sse_str`) through the same code path. |
| chrono | local (`clock`) | https://crates.io/crates/chrono | cox-app: local midnight and week start for the desktop's project spend (T37.29.3.3) |
| clap | local | https://crates.io/crates/clap | cox (CLI) |
| cpal | local | https://github.com/RustAudio/cpal | cox-voice: the default microphone (CoreAudio, ALSA, WASAPI) for push-to-talk, in memory only (T54.3, A123); Linux builds need the ALSA headers (libasound2-dev) |
| crossterm | local | https://crates.io/crates/crossterm | Terminal I/O |
| diesel | local | https://crates.io/crates/diesel | cox-store diesel/diesel_migrations pinned "2.2" per plan.md D9 resolve to the latest 2.x compatible release (2.3.x) on crates.io as of 2026-09-02; verified no semver-breaking API change vs. 2.2 for the sqlite backend used here. |
| diesel_migrations | local | https://crates.io/crates/diesel_migrations | SQLite migrations |
| diffy | local | https://crates.io/crates/diffy | Rust dependency |
| directories | local | https://crates.io/crates/directories | Rust dependency |
| dotenvy | local | https://crates.io/crates/dotenvy | T0.7: load local .env files without overriding the process environment. |
| eventsource-stream | local | https://crates.io/crates/eventsource-stream | Rust dependency |
| extism | local (default features off) | https://github.com/extism/extism | cox-plugin: the WASM plugin host (A52, T33.3); no ureq, no URL or file module loading |
| extism-pdk | local, `plugins/` guest workspace (default features off) | https://github.com/extism/rust-pdk | cox-plugin-sdk: the official Rust PDK the guest SDK wraps (exports, `cox:host/v1` imports, extism memory; T33.27) |
| figment | local | https://crates.io/crates/figment | Config loading |
| polib | local | https://crates.io/crates/polib | cox-i18n: parses the embedded gettext `.po` catalogs in pure Rust (no libintl; `docs/i18n.md`) |
| futures | local | https://crates.io/crates/futures | Rust dependency |
| globset | local | https://crates.io/crates/globset | Rust dependency |
| grep-regex | local | https://crates.io/crates/grep-regex | T3.3: plan.md names "grep-regex + grep-searcher sinks"; grep-regex (the RegexMatcher grep-searcher needs) was missing from this list. |
| grep-searcher | local | https://crates.io/crates/grep-searcher | Rust dependency |
| ignore | local | https://crates.io/crates/ignore | cox-tools |
| insta | local | https://crates.io/crates/insta | dev-deps |
| keyring | local | https://crates.io/crates/keyring | Rust dependency |
| landlock | local | https://crates.io/crates/landlock | Rust dependency |
| libfuzzer-sys | local | https://crates.io/crates/libfuzzer-sys | Rust dependency |
| libsqlite3-sys | local | https://crates.io/crates/libsqlite3-sys | Rust dependency |
| nix | local | https://crates.io/crates/nix | Rust dependency |
| nucleo | local | https://crates.io/crates/nucleo | Rust dependency |
| opentelemetry | local | https://crates.io/crates/opentelemetry | D16/T13: OTLP/HTTP keeps telemetry vendor-neutral (SigNoz, Jaeger, Grafana/Tempo and hosted collectors) without adding a backend SDK. |
| opentelemetry-appender-tracing | local | https://crates.io/crates/opentelemetry-appender-tracing | Rust dependency |
| opentelemetry-otlp | local | https://crates.io/crates/opentelemetry-otlp | Rust dependency |
| opentelemetry_sdk | local | https://crates.io/crates/opentelemetry_sdk | Rust dependency |
| pathdiff | local | https://crates.io/crates/pathdiff | Relative path between two paths |
| portable-pty | local | https://crates.io/crates/portable-pty | Rust dependency: the TUI e2e tests' PTY and, T51.3, `cox-app`'s terminal pane (0.9.0, the latest release, 2025-02-11) |
| predicates | local | https://crates.io/crates/predicates | Rust dependency |
| pretty_assertions | local | https://crates.io/crates/pretty_assertions | Rust dependency |
| proptest | local | https://crates.io/crates/proptest | Rust dependency |
| pulldown-cmark | local | https://crates.io/crates/pulldown-cmark | T5.3: plan.md says pulldown-cmark 0.10; 0.13 is the current line with the same Tag/TagEnd API. syntect without onig (pure-Rust fancy-regex engine). Lives in `cox-render` (T32.2). |
| ratatui | local | https://crates.io/crates/ratatui | cox-tui |
| reqwest | local | https://crates.io/crates/reqwest | cox-provider; cox-plugin: the host side of `cox_http` (T33.14.1) |
| url | local | https://crates.io/crates/url | cox-tools: LSP `file://` URI ↔ path (T41.3); `cox-app`'s browser tools parse a URL and pass only http/https (T51.7). 2.5.8, the latest release, 2026-01-05 |
| rmcp | local | https://crates.io/crates/rmcp | cox-mcp |
| rubato | local | https://github.com/HEnquist/rubato | cox-voice: the microphone's rate (often 44.1 or 48 kHz) to whisper's 16 kHz (T54.3, A123) |
| rstest | local | https://crates.io/crates/rstest | Rust dependency |
| schemars | local | https://crates.io/crates/schemars | Rust dependency |
| schemars 0.8 | local (build-dependency) | https://crates.io/crates/schemars | cox-provider `build.rs`: typify 0.8's `TypeSpace` takes schemars 0.8 schema types |
| seccompiler | local | https://crates.io/crates/seccompiler | Rust dependency |
| serde | local | https://crates.io/crates/serde | cox-protocol |
| serde_json | local | https://crates.io/crates/serde_json | preserve_order: agent-client-protocol-schema requires it, which unifies the feature into every workspace build anyway; pinning it here makes JSON key order (snapshots, rollout lines, ledger JSON) identical for per-crate and full-workspace runs instead of depending on the invocation. |
| serde_yaml | local | https://crates.io/crates/serde_yaml | cox-ext |
| sha2 | local | https://crates.io/crates/sha2 | Rust dependency |
| shlex | local | https://crates.io/crates/shlex | Rust dependency |
| similar | local | https://crates.io/crates/similar | Rust dependency |
| syntect | local | https://crates.io/crates/syntect | Rust dependency Lives in `cox-render` (T32.2). |
| sys-locale | local | https://crates.io/crates/sys-locale | cox-i18n: the OS UI languages when the env names none |
| two-face | local | https://crates.io/crates/two-face | T24.3: extended syntax definitions Lives in `cox-render` (T32.2). |
| terminal-colorsaurus | local | https://crates.io/crates/terminal-colorsaurus | T22.6: OSC 11 background colour query for `tui.theme = "auto"` Lives in `cox-render` (T32.2). |
| tempfile | local | https://crates.io/crates/tempfile | Rust dependency |
| thiserror | local | https://crates.io/crates/thiserror | Error enums |
| tiktoken-rs | local | https://crates.io/crates/tiktoken-rs | Rust dependency |
| tokio | local | https://crates.io/crates/tokio | cox-core |
| tokio-util | local | https://crates.io/crates/tokio-util | Rust dependency; `compat` feature (crates/cox, T35.13) bridges a spawned external agent's tokio stdio to the ACP client |
| toml_edit | local | https://crates.io/crates/toml_edit | Rust dependency |
| tracing | local | https://crates.io/crates/tracing | Logging |
| tracing-appender | local | https://crates.io/crates/tracing-appender | Rust dependency |
| tracing-opentelemetry | local | https://crates.io/crates/tracing-opentelemetry | Rust dependency |
| tracing-subscriber | local | https://crates.io/crates/tracing-subscriber | Rust dependency |
| tree-sitter | local | https://crates.io/crates/tree-sitter | Rust dependency |
| tree-sitter-bash | local | https://crates.io/crates/tree-sitter-bash | Rust dependency |
| tree-sitter-go | local | https://crates.io/crates/tree-sitter-go | Rust dependency |
| tree-sitter-python | local | https://crates.io/crates/tree-sitter-python | Rust dependency |
| tree-sitter-rust | local | https://crates.io/crates/tree-sitter-rust | Rust dependency |
| tree-sitter-typescript | local | https://crates.io/crates/tree-sitter-typescript | Rust dependency |
| tui-textarea-2 | local | https://crates.io/crates/tui-textarea-2 | Rust dependency |
| typify | local (build-dependency) | https://github.com/oxidecomputer/typify | cox-provider `build.rs`: Anthropic request and stream types generated from the vendored `schema/anthropic-openapi.json` (T30.10, T30.12) |
| ulid | local | https://crates.io/crates/ulid | Identifiers |
| unic-langid | local | https://crates.io/crates/unic-langid | cox-i18n: BCP 47 language identifiers for locale negotiation |
| unicode-width | local | https://crates.io/crates/unicode-width | Rust dependency |
| vt100 | local | https://crates.io/crates/vt100 | Rust dependency |
| wasmtime | local (`anyhow` feature only) | https://github.com/bytecodealliance/wasmtime | cox-plugin: the runtime under extism; declared only to enable the `anyhow` feature extism 1.30.0 needs with its default features off (T33.3) |
| whisper-rs | local (`tracing_backend`) | https://codeberg.org/tazz4843/whisper-rs | cox-voice: local push-to-talk transcription through whisper.cpp (MIT, built with cmake), behind `crates/cox`'s `voice` feature, off by default (T54.2, A123) |
| wiremock | local | https://crates.io/crates/wiremock | Rust dependency |
| trycmd | local | https://github.com/assert-rs/trycmd | dev-dep: `cox run -p` output fixtures (P48) |

## pub (`plugins/templates/dart`, `plugins/examples/dart`)

| Package | Where | Source | Why here |
| --- | --- | --- | --- |
| dart_mcp | local | https://pub.dev/packages/dart_mcp | T33.38: the official Dart MCP server/client SDK (`ToolsSupport`, `stdioChannel`) a Dart plugin's `[[mcp]]` server is built on, since Dart cannot emit a wasm module extism can load |

## uv (`evals/`)

| Package | Where | Source | Why here |
| --- | --- | --- | --- |
| pyyaml | local | https://github.com/yaml/pyyaml | Reads `evals/tasks/*.yaml` |
| tomli-w | local | https://github.com/hukkin/tomli-w | Writes scripted scenarios and the verify hook config |
| pytest | local (dev) | https://github.com/pytest-dev/pytest | Tests for the eval package |
| harbor | local (extra `tbench`) | https://github.com/laude-institute/harbor | Terminal-Bench 2.0 harness; `cox_evals.tbench:CoxAgent` is a Harbor agent |

## uv (`scripts/vendor/`)

| Package | Where | Source | Why here |
| --- | --- | --- | --- |
| pytest | local (dev) | https://github.com/pytest-dev/pytest | Tests for the vendor package |
| tomlkit | local | https://github.com/sdispater/tomlkit | Comment-preserving TOML edits for `cox-vendor models` (T30.20) |

## npm (`desktop/design`)

| Package | Where | Source | Why here |
| --- | --- | --- | --- |
| style-dictionary | local | https://github.com/style-dictionary/style-dictionary | T37.17: generates CoxUI's `Tokens.swift` and `Colors.xcassets` and the mockups' `tokens.css` from the DTCG token files (DS§2) |
| pixelmatch | local | https://github.com/mapbox/pixelmatch | T37.44.4: decides which pixels of a CoxUI snapshot differ from its Figma or mockup frame (`npm run diff`, DS§2) |
| sharp | local | https://github.com/lovell/sharp | T37.44.4: decodes, crops and resamples the snapshot and frame to one size and writes the diff PNG (`npm run diff`, DS§2) |

## SwiftPM (`desktop/macos/Packages`)

| Package | Where | Source | Why here |
| --- | --- | --- | --- |
| swift-collections | local (`CoxModel`) | https://github.com/apple/swift-collections | T37.16: `OrderedDictionary` keeps the timeline store in block order (research.md §9.5.6) |
| SwiftLintPlugins | local (every package under `desktop/macos/Packages`) | https://github.com/SimplyDanny/SwiftLintPlugins | T37.18: `SwiftLintBuildToolPlugin` lints each package's targets; version equals the SwiftLint pin in `mise.toml` |
| swift-snapshot-testing | local (`CoxUI` tests) | https://github.com/pointfreeco/swift-snapshot-testing | T37.19: image snapshots of the CoxUI Foundations and components (DS§9, A67) |
| SwiftTerm | local (`CoxPlatform`) | https://github.com/migueldeicaza/SwiftTerm | T51.5: the terminal pane's VT emulator and renderer, `TerminalView` only — the shell runs in Rust (research.md §9.5.2; 1.20.0, the latest release, 2026-08-18; MIT) |
| KeyboardShortcuts | local (the `Cox` app target, `desktop/macos/project.yml`) | https://github.com/sindresorhus/KeyboardShortcuts | T51.15: the two global hotkeys' recorder, registration and storage in Settings › General (DT§4.6; 3.1.0, the latest release, 2026-09-11; MIT) |
