# cox — task runner. Every target runs through `mise exec` so the pinned
# toolchain (mise.toml) is used, never whatever `cargo` happens to be on PATH.
# On Windows just has no `sh`; the desktop-windows recipes are PowerShell.
set windows-shell := ["powershell.exe", "-NoLogo", "-Command"]

check:
    bash scripts/leftovers.sh
    mise exec -- cargo fmt --check
    mise exec -- cargo clippy --workspace --all-targets -- -D warnings

# Only the tests a change can break (A99): the crates that own the files
# changed since REF, and every crate depending on them. REF defaults to the
# merge-base with origin/main; uncommitted and untracked files count. A
# Cargo.toml/Cargo.lock/.cargo/mise.toml/justfile change runs everything.
# `just test --changed-since HEAD`, `just test --dry-run` (print the command),
# other flags go to nextest.
[positional-arguments]
test *args:
    @uv run --no-project python scripts/changed_tests.py "$@"


# The tests that validate docs/ (config keys, subcommands, links, ide.md, generated config.md,
# the keymap table). CI's `docs` job runs only this on a docs-only pull request, with
# CARGO=cargo (Rust already comes from mise there).
docs-check:
    {{docs_cargo}} test --locked -p cox --test docs --test ide
    {{docs_cargo}} test --locked -p cox-protocol --lib config
    {{docs_cargo}} test --locked -p cox-tui --lib keymap_table_matches_docs

docs_cargo := env("CARGO", "mise exec -- cargo")
# The whole workspace, then the swarfr cleanup; CI runs the same suite.
check-all: && swarfr
    mise exec -- cargo nextest run --workspace

# The crates that build for Windows (P57, T57.1): the one list `just
# windows-check` and CI's `windows` job use. Each P57 card adds the crates it
# ports; T57.11 replaces the list with the whole workspace.
windows_crates := "cox-plugin-api cox-protocol cox-sanitize cox-i18n cox-models cox-permission cox-config cox-tokens cox-patch cox-search cox-syntax cox-web cox-sandbox cox-render cox-telemetry cox-provider-http cox-provider-testkit cox-provider-anthropic cox-provider-openai cox-provider cox-store cox-plugin cox-core cox-tui cox-tools cox-acp"

# `cargo check` and clippy over `windows_crates` for x64 and ARM64 Windows
# (A128 (4)), library targets only (tests join in T57.11). On Windows cargo
# runs natively (CI's `windows` job, whose rust action already made the
# mise.toml pin the active toolchain). Elsewhere through cargo-xwin
# (mise.toml) and its `clang` backend: clang-cl's `/imsvc` flags reach
# ring's aarch64 assembly, which plain clang builds, and fail it. The
# backend fetches a Windows sysroot into its cache on first use (several
# GB); the Windows std comes from `rustup target add`, run here rather than
# pinned in mise.toml so macOS and Linux CI never download it.
[script("bash")]
windows-check:
    set -euo pipefail
    targets=(x86_64-pc-windows-msvc aarch64-pc-windows-msvc)
    args=()
    for target in "${targets[@]}"; do args+=(--target "$target"); done
    for crate in {{windows_crates}}; do args+=(-p "$crate"); done
    if [ "{{os()}}" = windows ]; then
        run=(env) cargo=(cargo)
    else
        # mise exec puts each pin on PATH itself; its shims directory can
        # only add an inactive `clang` shim another project pinned, which
        # cargo-xwin would take as the compiler.
        PATH=$(printf %s "$PATH" | tr ':' '\n' | grep -v '/mise/shims$' | paste -sd: -)
        export XWIN_CROSS_COMPILER=clang
        run=(mise exec --) cargo=(mise exec -- cargo xwin)
    fi
    "${run[@]}" rustup target add "${targets[@]}"
    "${cargo[@]}" check "${args[@]}"
    "${cargo[@]}" clippy "${args[@]}" -- -D warnings

# The guest workspace (plugins/, PL§9): pure/host-target tests only — no
# wasm32 build here, that is CI's separate step (T33.40.2).
plugin-test:
    mise exec -- cargo test --manifest-path plugins/Cargo.toml --workspace

# Build a guest-language example (PL§13) with its own toolchain (installed
# from plugins/mise.toml) and run its ignored e2e test, e.g.
# `just plugin-examples dart`. The `plugin-examples` CI job runs this for
# every language; a missing toolchain there fails the job.
plugin-examples lang:
    #!/usr/bin/env sh
    set -eu
    case "{{lang}}" in
      dart)
        cd plugins && mise install dart@3.13.4
        cd examples/dart
        mise exec -- dart pub get
        mkdir -p build
        mise exec -- dart compile exe bin/server.dart -o build/example_dart
        ;;
      *)
        echo "plugin-examples: no {{lang}} example yet" >&2
        exit 1
        ;;
    esac
    cd "{{justfile_directory()}}"
    mise exec -- cargo nextest run -p cox --run-ignored only -E 'test(plugin_example_{{lang}})'

# Lossless cleanup of ./target (compress + dedupe); never deletes. A no-op without swarfr.
swarfr:
    #!/usr/bin/env sh
    command -v swarfr >/dev/null || { echo "swarfr not found; install it with: ketch install swarfr"; exit 0; }
    [ -d target ] || exit 0
    swarfr run target || test $? -eq 2

snap:
    mise exec -- cargo insta review

eval *args:
    uv run --project evals cox-evals {{args}}

# Tests for the eval package; no network, no key (the e2e ones need `cargo build -p cox`).
test-evals:
    uv run --project evals --extra tbench pytest evals/tests -q

# Vendor a file no package manager fetches (plan.md A48), e.g.
# `just vendor anthropic-spec` or `just vendor anthropic-spec --check`.
vendor *ARGS:
    uv run --project scripts/vendor cox-vendor {{ARGS}}

# Tests for the vendor package; no network.
vendor-test:
    uv run --project scripts/vendor pytest scripts/vendor/tests -q

# Token economy (R§4.6), then the PL§11 plugin timings over the Rust
# reference plugin (R§4.7; release, because they are latencies; needs the
# wasm32 target from mise.toml).
bench:
    mise exec -- cargo run -q -p cox --example bench
    mise exec -- cargo run -q --release -p cox --example plugin_bench

# Footprint benchmark (plan.md T30.2): cold start, first frame, replay RSS
# peak and binary size. `--write` refreshes scripts/footprint.json (commit
# the result); `--check` fails on a >20% regression vs the baseline (CI).
footprint *args:
    bash scripts/footprint.sh {{args}}

# Optimized single-binary build (fat LTO, one codegen unit, stripped) and its
# size. Same profile cargo-dist ships, so what you measure is what users get.
# Ask cargo for the target dir rather than assuming ./target — a shared
# build.target-dir in ~/.cargo/config.toml moves it.
release:
    mise exec -- cargo build --profile dist -p cox
    @ls -lh "$(mise exec -- cargo metadata --format-version 1 --no-deps | tr ',' '\n' | grep -o '"target_directory":"[^"]*"' | cut -d'"' -f4)/dist/cox" | awk '{print "cox  " $5}'

# The macOS app's Rust core (T37.15, DT§7): desktop/macos/build/CoxFFI.xcframework
# (aarch64-apple-darwin only, macOS 26.0) plus the generated Swift bindings.
desktop-xcframework:
    mise exec -- bash scripts/desktop/xcframework.sh

# The macOS app, Debug and ad-hoc signed (T37.32.1, DT§7): the XCFramework, then XcodeGen's
# thin Cox.xcodeproj from desktop/macos/project.yml, then desktop/macos/build/Cox.app. No
# signing identity; Developer ID and notarization are T37.32.2. Run it on a fixture with
# `desktop/macos/build/Cox.app/Contents/MacOS/Cox -CoxFixture desktop/macos/Fixtures/edit.json`.
desktop-app: desktop-xcframework
    mise exec -- bash scripts/desktop/app.sh

# The app as a DMG to hand out (T37.32.3): desktop/macos/build/Cox-<version>-<build>-debug-arm64.dmg,
# ad-hoc signed unless COX_SIGN_IDENTITY names a Developer ID Application identity. CI builds the
# same through the `desktop build` workflow.
desktop-dmg: desktop-app
    bash scripts/desktop/dmg.sh

# The Windows app (T58.3). Visual Studio is the Xcode counterpart: `just desktop-windows-open`
# opens desktop/windows/Cox.sln. Build and test run from that directory so global.json pins
# SDK 10 and the Microsoft.Testing.Platform runner. `just desktop-windows-run` builds Debug
# and starts the unpackaged window.
desktop-windows-open:
    powershell -NoProfile -File scripts/desktop/windows.ps1 open

desktop-windows:
    powershell -NoProfile -File scripts/desktop/windows.ps1 build

desktop-windows-test:
    powershell -NoProfile -File scripts/desktop/windows.ps1 test

desktop-windows-run:
    powershell -NoProfile -File scripts/desktop/windows.ps1 run

# The native apps' string resources from crates/cox-i18n/po (docs/i18n.md):
# Apple .strings/.stringsdict and Windows .resw under target/i18n/ (gitignored).
i18n-export *args:
    mise exec -- cargo run -q -p cox-i18n --bin po-export {{args}}

# Merges crates/cox-i18n/po/messages.pot into every <code>.po after the
# template changes (GNU gettext's msgmerge; `brew install gettext`).
i18n-update:
    for po in crates/cox-i18n/po/*.po; do msgmerge --quiet --update --backup=none "$po" crates/cox-i18n/po/messages.pot || exit 1; done

# Validates the catalogs with GNU gettext: msgfmt --check on each .po (header,
# plural forms, {name} placeholders), format checks on the template, and
# msgcmp that each .po holds exactly the template's messages.
i18n-check:
    msgfmt --check-format --output-file=/dev/null crates/cox-i18n/po/messages.pot
    for po in crates/cox-i18n/po/*.po; do msgfmt --check --output-file=/dev/null "$po" && msgcmp --use-untranslated "$po" crates/cox-i18n/po/messages.pot || exit 1; done

# The desktop design tokens (T37.17, DS§2): Style Dictionary regenerates CoxUI's
# Tokens.swift and Colors.xcassets and the mockups' tokens.css from
# desktop/design/tokens/*.json. CI runs the same build and fails on any diff.
desktop-tokens:
    cd desktop/design && mise exec -- npm ci --no-fund --no-audit && mise exec -- npm run build

# Every icon and logo raster from the SVGs in brand/logo/: the PNG exports in
# brand/logo/png/ (resvg, the mise pin), the macOS app icon (desktop/macos/App/AppIcon.icon)
# and CoxUI's vector mark. CI runs `--check` and fails on drift.
brand-icons *args:
    mise exec -- python3 scripts/brand_icons.py {{args}}

# $CARGO_HOME sizes (no deletes) and ./target
cache:
    mise exec -- cargo-cache
    du -sh target 2>/dev/null || echo "target: (missing)"

# drop extracted crate/git checkouts; keep archives
cache-autoclean:
    mise exec -- cargo-cache --autoclean

# Re-render docs/screenshots/*.svg from the whole-screen snapshot tests
# (crates/cox-tui/tests/screenshots.rs); the same frames insta compares.
screenshots:
    COX_SCREENSHOTS="{{justfile_directory()}}/docs/screenshots" mise exec -- cargo test -p cox-tui --test screenshots
