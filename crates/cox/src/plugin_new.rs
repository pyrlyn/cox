// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox plugin new <name> [--lang] [--dir] [--with …]` (PL§13): one pure
//! function, [`scaffold`], maps a name/language/capability list to the
//! files a fresh plugin package needs; [`write`] is the only place that
//! touches disk, and refuses an existing target directory rather than
//! overwrite it. Both are called from `crates/cox/src/main.rs`; T33.30's
//! TUI `/plugin new` calls the same [`scaffold`]/[`write`] pair, so there
//! is only one implementation of the mapping.
//!
//! Templates live in `plugins/templates/<lang>/*.tmpl`, embedded with
//! `include_str!` so no new templating dependency is needed. For
//! [`Lang::Rust`], a capability block is marked `cox:with=<cap>[,<cap>...]`
//! … `cox:end` on its own lines (the comment syntax around the markers does
//! not matter, since the block stripper only looks for the substrings) and
//! is kept only when one of its capabilities was chosen — the one
//! mechanism that keeps `plugin.toml`'s `[capabilities]`, the guest's stub
//! exports and its `register!` call from ever drifting apart.
//!
//! [`Lang::Dart`] (T33.38) has no such variance: Dart cannot emit a wasm
//! module extism can load (`docs/design/plugins.md` §13-14, `research.md`
//! §4.3.5 P44 — `dart compile wasm` still needs a JS bootstrap), so its
//! template is always the same shape, an `[[mcp]]` stdio server with no
//! wasm export. [`scaffold_dart`] refuses any `--with` capability other
//! than `tool`/`mcp` with [`PluginNewError::DartCapability`] instead of
//! stripping blocks.

use std::fs;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use cox_plugin_api::is_plugin_id;
use thiserror::Error;

/// One file `scaffold` produced, path relative to the package root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaffoldFile {
    /// Relative to the package root `write` is given.
    pub path: PathBuf,
    /// Its full contents.
    pub content: String,
}

/// `--lang`. [`Lang::Rust`] and [`Lang::Dart`] have templates; [`Lang::Go`]
/// and [`Lang::Kotlin`] are PL§13's remaining planned languages, kept here
/// so `--lang go` names a real, if not-yet-scaffolded, choice instead of
/// clap's generic "invalid value".
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum Lang {
    /// The reference language: full ABI, every capability.
    Rust,
    /// T33.34.
    Go,
    /// T33.36, only if T33.35's spike passes.
    Kotlin,
    /// T33.38: MCP-server only, no wasm export (`--with` accepts only
    /// `tool`/`mcp`).
    Dart,
}

/// `--with`, PL§13's capability list, one variant per value the flag
/// accepts (`status,panel,command,key,renderer,hook,event,tool,provider,
/// models,mcp`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum Capability {
    Status,
    Panel,
    Command,
    Key,
    Renderer,
    Hook,
    Event,
    Tool,
    Provider,
    Models,
    Mcp,
}

/// Why `cox plugin new` refused.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PluginNewError {
    /// `name` breaks the same `^[a-z][a-z0-9-]{1,23}$` rule
    /// `cox-plugin-api` validates a manifest's `id` against.
    #[error("plugin name {0:?} must match ^[a-z][a-z0-9-]{{1,23}}$ (PL§2)")]
    InvalidName(String),
    /// `--lang` named a language PL§13 plans but has not templated yet.
    #[error("--lang {0:?} has no template yet (PL§13); only rust and dart are scaffolded today")]
    UnsupportedLang(Lang),
    /// `--with` named a capability a Dart plugin cannot back: it needs a
    /// wasm export, and Dart cannot emit a wasm module extism can load
    /// (`docs/design/plugins.md` §13-14, `research.md` §4.3.5 P44).
    #[error(
        "--lang dart only supports --with tool,mcp: {0:?} needs a wasm export, and \
         dart cannot emit a wasm module extism can load (docs/design/plugins.md §13-14, \
         research.md §4.3.5 P44) — its only capability is an [[mcp]] stdio server"
    )]
    DartCapability(Capability),
    /// The target directory is already there; `cox plugin new` never
    /// overwrites (PL§13).
    #[error("{} already exists; cox plugin new never overwrites", .0.display())]
    DirExists(PathBuf),
    /// A file could not be written.
    #[error("{}: {}", .0.display(), .1)]
    Io(PathBuf, String),
}

/// The Rust template's files, in the order they should be written.
const RUST_TEMPLATES: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        include_str!("../../../plugins/templates/rust/Cargo.toml.tmpl"),
    ),
    (
        "plugin.toml",
        include_str!("../../../plugins/templates/rust/plugin.toml.tmpl"),
    ),
    (
        "src/lib.rs",
        include_str!("../../../plugins/templates/rust/src/lib.rs.tmpl"),
    ),
    (
        "tests/smoke.rs",
        include_str!("../../../plugins/templates/rust/tests/smoke.rs.tmpl"),
    ),
    (
        "justfile",
        include_str!("../../../plugins/templates/rust/justfile.tmpl"),
    ),
    (
        "README.md",
        include_str!("../../../plugins/templates/rust/README.md.tmpl"),
    ),
    (
        ".gitignore",
        include_str!("../../../plugins/templates/rust/gitignore.tmpl"),
    ),
];

/// The Dart template's files (T33.38): always the same shape, an
/// `[[mcp]]` stdio server with no wasm export — see the module header.
const DART_TEMPLATES: &[(&str, &str)] = &[
    (
        "pubspec.yaml",
        include_str!("../../../plugins/templates/dart/pubspec.yaml.tmpl"),
    ),
    (
        "plugin.toml",
        include_str!("../../../plugins/templates/dart/plugin.toml.tmpl"),
    ),
    (
        "bin/server.dart",
        include_str!("../../../plugins/templates/dart/bin/server.dart.tmpl"),
    ),
    (
        "test/smoke_test.dart",
        include_str!("../../../plugins/templates/dart/test/smoke_test.dart.tmpl"),
    ),
    (
        "justfile",
        include_str!("../../../plugins/templates/dart/justfile.tmpl"),
    ),
    (
        "README.md",
        include_str!("../../../plugins/templates/dart/README.md.tmpl"),
    ),
    (
        ".gitignore",
        include_str!("../../../plugins/templates/dart/gitignore.tmpl"),
    ),
];

/// Maps `(name, lang, with)` to the files a fresh plugin package needs
/// (PL§13). Pure: no filesystem access, so a caller (the CLI, and T33.30's
/// TUI picker) can show what would be written, or refuse a name, before
/// touching disk.
pub fn scaffold(
    name: &str,
    lang: Lang,
    with: &[Capability],
) -> Result<Vec<ScaffoldFile>, PluginNewError> {
    if !is_plugin_id(name) {
        return Err(PluginNewError::InvalidName(name.to_string()));
    }
    let crate_name = name.replace('-', "_");
    let mut with: Vec<Capability> = with.to_vec();
    with.sort();
    with.dedup();

    match lang {
        Lang::Rust => scaffold_rust(name, &crate_name, &with),
        Lang::Dart => scaffold_dart(name, &crate_name, &with),
        Lang::Go | Lang::Kotlin => Err(PluginNewError::UnsupportedLang(lang)),
    }
}

fn scaffold_rust(
    id: &str,
    crate_name: &str,
    with: &[Capability],
) -> Result<Vec<ScaffoldFile>, PluginNewError> {
    let summary = with_summary(with);
    Ok(RUST_TEMPLATES
        .iter()
        .map(|(path, tmpl)| ScaffoldFile {
            path: PathBuf::from(path),
            content: render(tmpl, id, crate_name, &summary, with),
        })
        .collect())
}

/// PL§13/T33.38: refuses any capability that is not `tool`/`mcp` — both
/// scaffold the same `[[mcp]]` server, since Dart has no wasm-backed
/// capability to choose between — then renders the fixed template.
fn scaffold_dart(
    id: &str,
    crate_name: &str,
    with: &[Capability],
) -> Result<Vec<ScaffoldFile>, PluginNewError> {
    if let Some(bad) = with
        .iter()
        .find(|c| !matches!(c, Capability::Tool | Capability::Mcp))
    {
        return Err(PluginNewError::DartCapability(*bad));
    }
    let summary = with_summary(with);
    let class_name = pascal_case(crate_name);
    Ok(DART_TEMPLATES
        .iter()
        .map(|(path, tmpl)| ScaffoldFile {
            path: PathBuf::from(path),
            content: render_dart(tmpl, id, crate_name, &summary, &class_name),
        })
        .collect())
}

/// The human-readable `--with` list every template's README embeds, e.g.
/// "status, hook" or "no extra capabilities". Shared by every language so
/// the wording never drifts between templates.
fn with_summary(with: &[Capability]) -> String {
    if with.is_empty() {
        "no extra capabilities".to_string()
    } else {
        with.iter()
            .map(|c| {
                c.to_possible_value()
                    .expect("no skipped variants")
                    .get_name()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// `snake_case` (a plugin id's `-` already became `_` in `crate_name`) to
/// `PascalCase`, for the Dart server class name — Dart requires
/// `UpperCamelCase` type names, and `{{id}}` is `^[a-z][a-z0-9-]{1,23}$`.
fn pascal_case(snake: &str) -> String {
    snake
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Strips every `cox:with=<cap>[,<cap>...]` … `cox:end` block whose
/// capability list does not intersect `with` (`render`'s `status`/`panel`
/// arms nest inside the function's own `status,panel` block, so this is a
/// stack, not a single flag: a `cox:end` closes only its own marker, and a
/// line is dropped when any enclosing level is skipping), then substitutes
/// the `{{id}}`/`{{crate_name}}`/`{{with_summary}}` placeholders.
fn render(
    tmpl: &str,
    id: &str,
    crate_name: &str,
    with_summary: &str,
    with: &[Capability],
) -> String {
    let mut out = String::new();
    let mut skip_stack: Vec<bool> = Vec::new();
    for line in tmpl.lines() {
        if let Some((_, rest)) = line.split_once("cox:with=") {
            let names = rest.split_whitespace().next().unwrap_or("");
            let keep = names
                .split(',')
                .any(|n| Capability::from_str(n, true).is_ok_and(|c| with.contains(&c)));
            skip_stack.push(!keep);
            continue;
        }
        if line.contains("cox:end") {
            skip_stack.pop();
            continue;
        }
        if !skip_stack.contains(&true) {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.replace("{{id}}", id)
        .replace("{{crate_name}}", crate_name)
        .replace("{{with_summary}}", with_summary)
}

/// The Dart template's substitution pass (T33.38): no `cox:with` blocks to
/// strip (the template has one fixed shape, see the module header), just
/// the placeholders, plus `{{class_name}}` for the server's Dart class.
fn render_dart(
    tmpl: &str,
    id: &str,
    crate_name: &str,
    with_summary: &str,
    class_name: &str,
) -> String {
    tmpl.replace("{{id}}", id)
        .replace("{{crate_name}}", crate_name)
        .replace("{{with_summary}}", with_summary)
        .replace("{{class_name}}", class_name)
}

/// Writes `files` under `dir`, which must not already exist (PL§13: "An
/// existing target directory is an error, never an overwrite").
pub fn write(dir: &Path, files: &[ScaffoldFile]) -> Result<(), PluginNewError> {
    if dir.exists() {
        return Err(PluginNewError::DirExists(dir.to_path_buf()));
    }
    for file in files {
        let full = dir.join(&file.path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| PluginNewError::Io(full.clone(), e.to_string()))?;
        }
        fs::write(&full, &file.content)
            .map_err(|e| PluginNewError::Io(full.clone(), e.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_name_that_breaks_the_plugin_id_rule() {
        for bad in ["Demo", "1demo", "d", "de_mo", ""] {
            assert_eq!(
                scaffold(bad, Lang::Rust, &[]),
                Err(PluginNewError::InvalidName(bad.to_string())),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn accepts_a_valid_plugin_id() {
        assert!(scaffold("demo", Lang::Rust, &[]).is_ok());
    }

    #[test]
    fn refuses_a_language_without_a_template_yet() {
        for lang in [Lang::Go, Lang::Kotlin] {
            assert_eq!(
                scaffold("demo", lang, &[]),
                Err(PluginNewError::UnsupportedLang(lang))
            );
        }
    }

    #[test]
    fn new_dart_rejects_status_with_reason() {
        let err = scaffold("demo", Lang::Dart, &[Capability::Status]).unwrap_err();
        assert_eq!(err, PluginNewError::DartCapability(Capability::Status));
        let msg = err.to_string();
        assert!(msg.contains("wasm"), "{msg}");
        assert!(msg.contains("tool,mcp"), "{msg}");
    }

    #[test]
    fn dart_scaffold_accepts_only_tool_and_mcp() {
        for cap in [Capability::Tool, Capability::Mcp] {
            assert!(
                scaffold("demo", Lang::Dart, &[cap]).is_ok(),
                "--with {cap:?} should be accepted for --lang dart"
            );
        }
        assert!(
            scaffold("demo", Lang::Dart, &[]).is_ok(),
            "an empty --with should still scaffold the mcp server"
        );
        for cap in [
            Capability::Status,
            Capability::Panel,
            Capability::Command,
            Capability::Key,
            Capability::Renderer,
            Capability::Hook,
            Capability::Event,
            Capability::Provider,
            Capability::Models,
        ] {
            assert_eq!(
                scaffold("demo", Lang::Dart, &[cap]),
                Err(PluginNewError::DartCapability(cap)),
                "{cap:?} should be refused for --lang dart"
            );
        }
    }

    #[test]
    fn dart_scaffold_writes_an_mcp_only_manifest_with_no_wasm_capabilities() {
        let files = scaffold("demo", Lang::Dart, &[Capability::Mcp]).unwrap();
        let manifest = &files
            .iter()
            .find(|f| f.path == Path::new("plugin.toml"))
            .unwrap()
            .content;
        assert!(manifest.contains("[[mcp]]"), "{manifest}");
        assert!(
            !manifest.contains("[capabilities]"),
            "a Dart plugin has no wasm-backed capabilities: {manifest}"
        );
        assert!(
            !manifest.contains("wasm ="),
            "a wasm-less mcp-only package ships no wasm line: {manifest}"
        );
        assert!(
            !manifest.contains("cox:with"),
            "a marker leaked: {manifest}"
        );

        // PL§13/§14 (T33.38): the host's own validator must accept this
        // wasm-less, mcp-only manifest exactly as scaffolded, so the
        // scaffolder and the loader never drift apart on "mcp-only".
        let tmp =
            std::env::temp_dir().join(format!("cox-plugin-new-test-dart-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        write(&tmp, &files).unwrap();
        let (parsed, _) =
            cox_plugin::discover::load_manifest(&tmp, &tmp.join("plugin.toml"), Some("demo"))
                .expect("the scaffolded manifest loads and validates");
        assert_eq!(parsed.wasm, None);
        let _ = fs::remove_dir_all(&tmp);

        let server = &files
            .iter()
            .find(|f| f.path == Path::new("bin/server.dart"))
            .unwrap()
            .content;
        assert!(server.contains("class Demo"), "{server}");
        assert!(
            !server.contains("{{"),
            "an unrendered placeholder leaked: {server}"
        );
    }

    #[test]
    fn plugin_toml_capabilities_match_with_exactly() {
        let files = scaffold("demo", Lang::Rust, &[Capability::Status, Capability::Hook]).unwrap();
        let manifest = &files
            .iter()
            .find(|f| f.path == Path::new("plugin.toml"))
            .unwrap()
            .content;
        assert!(manifest.contains("status = true"));
        assert!(manifest.contains(r#"hooks = ["PostToolUse"]"#));
        assert!(!manifest.contains("panel = true"));
        assert!(!manifest.contains("commands = true"));
        assert!(!manifest.contains("[[provider]]"));
        assert!(!manifest.contains("[[mcp]]"));
        assert!(!manifest.contains("[[models]]"));
    }

    /// The one general check a per-capability `contains`/`!contains` test
    /// cannot: a stripped block leaving a dangling fragment of a sibling
    /// block behind (`render`'s nested `status`/`panel` arms did exactly
    /// this before the block stripper became a stack).
    fn assert_balanced_braces(rust: &str) {
        let (open, close) = (rust.matches('{').count(), rust.matches('}').count());
        assert_eq!(open, close, "unbalanced braces:\n{rust}");
    }

    #[test]
    fn bare_scaffold_declares_no_capabilities() {
        let files = scaffold("demo", Lang::Rust, &[]).unwrap();
        let manifest = &files
            .iter()
            .find(|f| f.path == Path::new("plugin.toml"))
            .unwrap()
            .content;
        assert!(
            !manifest.contains("cox:with"),
            "a marker leaked: {manifest}"
        );
        assert!(!manifest.contains("= true"));
        let lib = &files
            .iter()
            .find(|f| f.path == Path::new("src/lib.rs"))
            .unwrap()
            .content;
        assert!(lib.contains("fn init"));
        assert!(!lib.contains("fn hook"));
        assert!(!lib.contains("fn render("));
        assert!(
            !lib.contains("Widget::Text"),
            "a dangling render fragment leaked"
        );
        assert!(!lib.contains("cox:with"), "a marker leaked: {lib}");
        assert_balanced_braces(lib);
    }

    #[test]
    fn lib_rs_keeps_only_the_chosen_stub_exports() {
        let files = scaffold("demo", Lang::Rust, &[Capability::Tool]).unwrap();
        let lib = &files
            .iter()
            .find(|f| f.path == Path::new("src/lib.rs"))
            .unwrap()
            .content;
        assert!(lib.contains("fn tool_call"));
        assert!(lib.contains("tool_call => tool_call"));
        assert!(!lib.contains("fn hook"));
        assert!(!lib.contains("fn render("));
        assert_balanced_braces(lib);
    }

    #[test]
    fn render_keeps_only_its_chosen_nested_arm() {
        let only_status = scaffold("demo", Lang::Rust, &[Capability::Status]).unwrap();
        let lib = &only_status
            .iter()
            .find(|f| f.path == Path::new("src/lib.rs"))
            .unwrap()
            .content;
        assert!(lib.contains("fn render("));
        assert!(lib.contains("StatusLeft | Slot::StatusRight"));
        assert!(!lib.contains("Slot::Panel"));
        assert_balanced_braces(lib);

        let only_panel = scaffold("demo", Lang::Rust, &[Capability::Panel]).unwrap();
        let lib = &only_panel
            .iter()
            .find(|f| f.path == Path::new("src/lib.rs"))
            .unwrap()
            .content;
        assert!(lib.contains("fn render("));
        assert!(lib.contains("Slot::Panel"));
        assert!(!lib.contains("StatusLeft"));
        assert_balanced_braces(lib);

        let both = scaffold("demo", Lang::Rust, &[Capability::Status, Capability::Panel]).unwrap();
        let lib = &both
            .iter()
            .find(|f| f.path == Path::new("src/lib.rs"))
            .unwrap()
            .content;
        assert!(lib.contains("StatusLeft"));
        assert!(lib.contains("Slot::Panel"));
        assert_balanced_braces(lib);
    }

    #[test]
    fn write_refuses_an_existing_directory() {
        let tmp = std::env::temp_dir().join(format!("cox-plugin-new-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let err = write(&tmp, &[]).unwrap_err();
        assert_eq!(err, PluginNewError::DirExists(tmp.clone()));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn write_creates_every_file_under_dir() {
        let tmp =
            std::env::temp_dir().join(format!("cox-plugin-new-test-w-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let files = scaffold("demo", Lang::Rust, &[Capability::Status]).unwrap();
        write(&tmp, &files).unwrap();
        for file in &files {
            assert!(tmp.join(&file.path).is_file(), "{}", file.path.display());
        }
        let _ = fs::remove_dir_all(&tmp);
    }
}
