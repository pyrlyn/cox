// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Output styles (T67.12): markdown files that change tone without touching
//! the cached prefix. The core appends the chosen body after the cache
//! breakpoint; this module only finds the files, because `cox-core` reads none.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::frontmatter;

/// One output style. `body` is the markdown the model sees once `/style`
/// selects it; the filename is the name when the header omits one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Style {
    pub name: String,
    pub description: String,
    pub body: String,
    pub path: PathBuf,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Discovered {
    /// Later directories replace an earlier style of the same name, so a
    /// project `.cox` file wins over `.claude`, which wins over `~/.cox`.
    pub styles: Vec<Style>,
    pub notices: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Header {
    name: Option<String>,
    description: Option<String>,
}

/// `~/.cox/output-styles`, then `<project>/.claude/output-styles`, then
/// `<project>/.cox/output-styles`. The card names those three places and
/// no others.
pub fn style_dirs(cox_home: Option<&Path>, project: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = cox_home {
        dirs.push(home.join("output-styles"));
    }
    if let Some(project) = project {
        dirs.push(project.join(".claude").join("output-styles"));
        dirs.push(project.join(".cox").join("output-styles"));
    }
    dirs
}

/// Reads every `*.md` in each directory. A missing directory is empty. A
/// file with no frontmatter is a plain body (the name is the file stem). A
/// broken header or a name the palette cannot round-trip is skipped with a
/// notice, never fatal.
pub fn discover(dirs: &[PathBuf]) -> Discovered {
    let mut found = Discovered::default();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "md") && path.is_file())
            .collect();
        paths.sort();
        for path in paths {
            match parse_style(&path) {
                Ok(style) => {
                    found.styles.retain(|kept| kept.name != style.name);
                    found.styles.push(style);
                }
                Err(reason) => found
                    .notices
                    .push(format!("output style {} skipped: {reason}", path.display())),
            }
        }
    }
    found
}

fn parse_style(path: &Path) -> Result<Style, String> {
    let text = fs::read_to_string(path).map_err(|err| err.to_string())?;
    let stem = path
        .file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or("no file name")?;
    let (header, body) = match frontmatter::parse::<Header>(&text) {
        Ok(parsed) => parsed,
        Err(frontmatter::FrontmatterError::Missing) => (Header::default(), text.as_str()),
        Err(err) => return Err(err.to_string()),
    };
    let name = header.name.unwrap_or(stem);
    if !valid_name(&name) {
        return Err(format!(
            "name `{name}` must be 1–64 letters, digits, spaces, hyphens or underscores"
        ));
    }
    let body = body.trim().to_string();
    if body.is_empty() {
        return Err("empty body".into());
    }
    Ok(Style {
        name,
        description: header.description.unwrap_or_default(),
        body,
        path: path.to_path_buf(),
    })
}

/// Names travel in `/style <name>` and inside an attribute, so they stay
/// a single token the command parser can join back from words.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with(' ')
        && !name.ends_with(' ')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) {
        fs::create_dir_all(dir).expect("dir");
        fs::write(dir.join(name), body).expect("write");
    }

    /// Project `.cox` replaces `.claude`, which replaces the home file of
    /// the same name. A header-less file keeps its stem.
    #[test]
    fn project_cox_style_replaces_claude_and_home() {
        let root = tempfile::tempdir().expect("temp");
        let home = root.path().join("home");
        let project = root.path().join("project");
        write(
            &home.join("output-styles"),
            "terse.md",
            "---\nname: terse\ndescription: from home\n---\nHome body.\n",
        );
        write(
            &project.join(".claude").join("output-styles"),
            "terse.md",
            "---\ndescription: from claude\n---\nClaude body.\n",
        );
        write(
            &project.join(".cox").join("output-styles"),
            "terse.md",
            "Project body.\n",
        );
        write(
            &project.join(".cox").join("output-styles"),
            "Explanatory.md",
            "---\nname: Explanatory\ndescription: says why\n---\nExplain.\n",
        );
        let found = discover(&style_dirs(Some(&home), Some(&project)));
        assert!(found.notices.is_empty(), "{:?}", found.notices);
        let terse = found
            .styles
            .iter()
            .find(|style| style.name == "terse")
            .expect("terse");
        assert_eq!(terse.body, "Project body.");
        assert_eq!(terse.description, "");
        let explanatory = found
            .styles
            .iter()
            .find(|style| style.name == "Explanatory")
            .expect("Explanatory");
        assert_eq!(explanatory.description, "says why");
        assert_eq!(explanatory.body, "Explain.");
    }

    #[test]
    fn broken_header_is_skipped_with_a_notice() {
        let root = tempfile::tempdir().expect("temp");
        let dir = root.path().join("output-styles");
        write(&dir, "bad.md", "---\nname: [unterminated\n---\nBody.\n");
        write(&dir, "ok.md", "Keep this.\n");
        let found = discover(&style_dirs(Some(root.path()), None));
        assert_eq!(found.styles.len(), 1);
        assert_eq!(found.styles[0].name, "ok");
        assert_eq!(found.notices.len(), 1);
        assert!(found.notices[0].contains("bad.md"), "{}", found.notices[0]);
    }
}
