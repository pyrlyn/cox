// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T12.3: `docs/config.md` documents every key in `config/default.toml`.
//! T52.22: every `cox` subcommand is named in the docs, and the docs' relative
//! links and the repository paths `docs/app-server.md` cites resolve.
//! T33.45.10: `docs/ru/plugins.md` and `docs/uk/plugins.md` follow the English
//! `docs/plugins.md` heading for heading, block for block.

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

/// The translated `docs/<lang>/*.md` files with their text.
fn localized_docs() -> Vec<(PathBuf, String)> {
    let mut out: Vec<_> = LANGS
        .iter()
        .filter_map(|lang| std::fs::read_dir(root().join("docs").join(lang)).ok())
        .flatten()
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

#[test]
fn docs_relative_links_resolve() {
    let broken: Vec<_> = docs()
        .iter()
        .chain(localized_docs().iter())
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

/// The languages with a `docs/<lang>/` tree (`docs/i18n.md` lists the UI ones).
const LANGS: [&str; 2] = ["ru", "uk"];

/// Headings (level and text) and fenced blocks (fence lines included) of a
/// Markdown page; a `#` line inside a fence is code, not a heading.
fn outline(text: &str) -> (Vec<(usize, String)>, Vec<Vec<String>>) {
    let (mut headings, mut blocks, mut open) = (Vec::new(), Vec::new(), None::<Vec<String>>);
    for line in text.lines() {
        if line.starts_with("```") {
            match open.take() {
                Some(mut block) => {
                    block.push(line.to_owned());
                    blocks.push(block);
                }
                None => open = Some(vec![line.to_owned()]),
            }
        } else if let Some(block) = open.as_mut() {
            block.push(line.to_owned());
        } else if let Some((hashes, title)) = line.split_once(' ')
            && !hashes.is_empty()
            && hashes.chars().all(|c| c == '#')
        {
            headings.push((hashes.len(), title.to_owned()));
        }
    }
    (headings, blocks)
}

/// GitHub's anchor for each heading: lowercase, spaces to hyphens, other
/// punctuation dropped, a repeat numbered `-1`, `-2`.
fn anchors(headings: &[(usize, String)]) -> Vec<String> {
    let mut seen = std::collections::HashMap::<String, usize>::new();
    headings
        .iter()
        .map(|(_, title)| {
            let slug: String = title
                .to_lowercase()
                .chars()
                .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
                .map(|c| if c == ' ' { '-' } else { c })
                .collect();
            let n = seen.entry(slug.clone()).or_default();
            let anchor = if *n == 0 { slug } else { format!("{slug}-{n}") };
            *n += 1;
            anchor
        })
        .collect()
}

/// `](#anchor)` targets.
fn page_anchors(text: &str) -> Vec<&str> {
    text.match_indices("](#")
        .filter_map(|(at, _)| {
            let rest = &text[at + 3..];
            Some(&rest[..rest.find(')')?])
        })
        .collect()
}

/// The generated field-index bullets, which are identifiers and stay English.
fn index_bullets(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|line| line.starts_with("- manifest `") || line.starts_with("- ABI `"))
        .collect()
}

/// Everything a translation must share with its English source: the heading
/// levels, every fenced block byte for byte, the field index, and the
/// relative links (one `../` deeper, plus the back-link to the source).
fn assert_translation_matches(english: &str, translated: &str, lang: &str) {
    let (en_headings, en_blocks) = outline(english);
    let (tr_headings, tr_blocks) = outline(translated);
    let levels = |h: &[(usize, String)]| h.iter().map(|(level, _)| *level).collect::<Vec<_>>();
    assert_eq!(
        levels(&tr_headings),
        levels(&en_headings),
        "{lang}: heading levels differ from docs/plugins.md"
    );
    assert_eq!(tr_blocks, en_blocks, "{lang}: a fenced block differs");
    assert_eq!(
        index_bullets(translated),
        index_bullets(english),
        "{lang}: the API field index differs"
    );
    let own = anchors(&tr_headings);
    let dangling: Vec<_> = page_anchors(translated)
        .into_iter()
        .filter(|anchor| !own.iter().any(|a| a == anchor))
        .collect();
    assert!(dangling.is_empty(), "{lang}: dangling anchors {dangling:?}");
    let mut links: Vec<String> = relative_links(translated)
        .into_iter()
        .map(str::to_owned)
        .collect();
    links.sort();
    links.dedup();
    let mut expected: Vec<String> = relative_links(english)
        .into_iter()
        .map(|target| format!("../{target}"))
        .chain(["../plugins.md".to_owned()])
        .collect();
    expected.sort();
    expected.dedup();
    assert_eq!(links, expected, "{lang}: relative links differ");
}

#[test]
fn plugin_docs_translations_match_the_english_page() {
    let english = std::fs::read_to_string(root().join("docs/plugins.md")).expect("plugins.md");
    for lang in LANGS {
        let path = root().join("docs").join(lang).join("plugins.md");
        let translated = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        assert_translation_matches(&english, &translated, lang);
    }
}

#[test]
fn a_translation_missing_a_heading_fails_the_parity_check() {
    let english = "# T\n\n## A\n\n```sh\n# not a heading\n```\n\n## B\n";
    let whole = "# Т\n\n## А\n\n```sh\n# not a heading\n```\n\n## Б\n";
    let short = "# Т\n\n## А\n\n```sh\n# not a heading\n```\n";
    let edited = "# Т\n\n## А\n\n```sh\n# translated\n```\n\n## Б\n";
    let back = "[source](../plugins.md)\n";
    let check = |translated: String| {
        std::panic::catch_unwind(|| assert_translation_matches(english, &translated, "xx")).is_ok()
    };
    assert!(check(format!("{whole}{back}")));
    assert!(!check(format!("{short}{back}")));
    assert!(!check(format!("{edited}{back}")));
}
