// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T12.3: `docs/config.md` documents every key in `config/default.toml`.
//! T52.22: every `cox` subcommand is named in the docs, and the docs' relative
//! links and the repository paths `docs/app-server.md` cites resolve.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../")
}

/// The top-level `docs/*.md` files with their text.
fn docs() -> Vec<(PathBuf, String)> {
    let mut out: Vec<_> = std::fs::read_dir(root().join("docs"))
        .expect("docs dir")
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .map(|path| {
            let text = std::fs::read_to_string(&path).expect("doc text");
            (path, text)
        })
        .collect();
    out.sort();
    out
}

fn collect(item: &toml_edit::Item, prefix: String, out: &mut Vec<String>) {
    match item {
        toml_edit::Item::Table(table) => {
            for (key, child) in table.iter() {
                let dotted = if prefix.is_empty() {
                    key.into()
                } else {
                    format!("{prefix}.{key}")
                };
                collect(child, dotted, out);
            }
        }
        toml_edit::Item::Value(_) => out.push(prefix),
        _ => {}
    }
}

#[test]
fn docs_config_covers_every_key() {
    let root = root();
    let toml = std::fs::read_to_string(root.join("config/default.toml")).expect("default.toml");
    let md = std::fs::read_to_string(root.join("docs/config.md")).expect("config.md");
    let doc: toml_edit::DocumentMut = toml.parse().expect("valid toml");
    let mut keys = Vec::new();
    collect(doc.as_item(), String::new(), &mut keys);
    assert!(!keys.is_empty(), "no keys parsed");
    // The reference groups by `## [section]` with short `` `key` `` bullets.
    let missing: Vec<_> = keys
        .iter()
        .filter(|k| {
            let (section, short) = k.rsplit_once('.').unwrap_or(("", k.as_str()));
            !md.contains(&format!("## `[{section}]`")) || !md.contains(&format!("`{short}`"))
        })
        .collect();
    assert!(missing.is_empty(), "undocumented keys: {missing:?}");
}

/// The subcommand names `cox --help` lists, `help` aside.
fn subcommands() -> Vec<String> {
    let home = tempfile::tempdir().expect("scratch home");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cox"))
        .arg("--help")
        .env("COX_HOME", home.path())
        .output()
        .expect("run cox --help");
    assert!(out.status.success(), "cox --help failed");
    let help = String::from_utf8(out.stdout).expect("utf-8 help");
    help.lines()
        .skip_while(|line| *line != "Commands:")
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| *name != "help")
        .map(str::to_owned)
        .collect()
}

#[test]
fn docs_name_every_subcommand() {
    let commands = subcommands();
    assert!(
        commands.iter().any(|c| c == "run"),
        "help parsed: {commands:?}"
    );
    let text: String = docs().into_iter().map(|(_, text)| text).collect();
    let missing: Vec<_> = commands
        .iter()
        .filter(|name| {
            let needle = format!("cox {name}");
            !text.match_indices(&needle).any(|(at, _)| {
                // `cox run` must not be satisfied by `cox runner`.
                text[at + needle.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_alphanumeric() && c != '-')
            })
        })
        .collect();
    assert!(missing.is_empty(), "subcommands no doc names: {missing:?}");
}

/// Every `](target)` in `text` that is not a URL or a same-page anchor, with
/// its `#fragment` cut.
fn relative_links(text: &str) -> Vec<&str> {
    text.match_indices("](")
        .filter_map(|(at, _)| {
            let rest = &text[at + 2..];
            let target = &rest[..rest.find(')')?];
            let target = target.split('#').next().unwrap_or(target);
            let external = target.contains("://") || target.starts_with("mailto:");
            (!external && !target.is_empty()).then_some(target)
        })
        .collect()
}

#[test]
fn docs_relative_links_resolve() {
    let broken: Vec<_> = docs()
        .iter()
        .flat_map(|(path, text)| {
            let dir = path.parent().expect("docs dir").to_path_buf();
            relative_links(text)
                .into_iter()
                .filter(move |target| !dir.join(target).exists())
                .map(move |target| format!("{}: {target}", path.display()))
        })
        .collect();
    assert!(broken.is_empty(), "broken links: {broken:?}");
}

#[test]
fn app_server_doc_cites_paths_that_exist() {
    let text = std::fs::read_to_string(root().join("docs/app-server.md")).expect("app-server.md");
    let cited: Vec<_> = text
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|span| {
            !span.contains(' ') && ["docs/", "crates/"].iter().any(|dir| span.starts_with(dir))
        })
        .collect();
    assert!(!cited.is_empty(), "no repository paths found");
    let missing: Vec<_> = cited
        .iter()
        .filter(|span| !root().join(span).exists())
        .collect();
    assert!(
        missing.is_empty(),
        "cited paths that do not exist: {missing:?}"
    );
}

#[test]
fn relative_links_skip_urls_and_anchors() {
    let text = "[a](config.md#x) [b](https://x.dev) [c](#top) [d](screenshots/)";
    assert_eq!(relative_links(text), ["config.md", "screenshots/"]);
}
