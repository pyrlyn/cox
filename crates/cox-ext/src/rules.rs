// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Path-scoped project rules (T67.6). `.cox/rules/*.md` and
//! `.claude/rules/*.md` stay out of the cached prefix until a `read`,
//! `edit` or `write` touches a file their globs name. Separate from
//! instruction files, which load once into `system[2]`, because a rule
//! that joined that block would change the cache-stable prefix the first
//! time a matching file was touched.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::Deserialize;

use crate::frontmatter;

/// One rule file that parsed. `body` is already sanitized and framed; it
/// is repository text, not a system instruction.
#[derive(Debug, Clone)]
pub struct Rule {
    pub globs: Vec<String>,
    pub body: String,
    pub source: String,
    set: GlobSet,
}

/// Rules plus the files that were skipped. A broken file is a warning, never
/// fatal (D14).
#[derive(Debug, Default)]
pub struct Loaded {
    pub rules: Vec<Rule>,
    pub notices: Vec<String>,
}

#[derive(Deserialize, Default)]
struct Header {
    paths: Option<frontmatter::Value>,
}

const RULE_DIRS: &[&str] = &[".cox/rules", ".claude/rules"];

/// Reads every rule file under `roots`. Called again on each request so a
/// file added during the session is visible; the caller keeps the
/// once-per-session injection.
pub fn load_rules(roots: &[PathBuf]) -> Loaded {
    let mut loaded = Loaded::default();
    let mut seen = HashSet::new();
    for root in roots {
        let Ok(root) = fs::canonicalize(root) else {
            continue;
        };
        for dir in RULE_DIRS {
            collect_dir(&root, &root.join(dir), &mut seen, &mut loaded);
        }
    }
    loaded
}

/// Rules whose globs match any touched path. Pure: no I/O.
pub fn rules_for<'a>(touched: &[String], rules: &'a [Rule]) -> Vec<&'a Rule> {
    rules
        .iter()
        .filter(|rule| touched.iter().any(|path| rule.matches(path)))
        .collect()
}

fn collect_dir(root: &Path, dir: &Path, seen: &mut HashSet<PathBuf>, loaded: &mut Loaded) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            loaded.notices.push(format!("{}: {e}", dir.display()));
            return;
        }
    };
    let mut files = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
            files.push(path);
        }
    }
    files.sort();
    for path in files {
        let Some(canon) = file_under_root(root, &path) else {
            loaded.notices.push(format!(
                "{}: skipped (not a file under the project root)",
                path.display()
            ));
            continue;
        };
        if !seen.insert(canon.clone()) {
            continue;
        }
        match read_rule(root, &canon) {
            Ok(rule) => loaded.rules.push(rule),
            Err(notice) => loaded.notices.push(notice),
        }
    }
}

/// A symlink out of the root would otherwise put a file the repository does
/// not contain into the prompt.
fn file_under_root(root: &Path, path: &Path) -> Option<PathBuf> {
    let canon = fs::canonicalize(path).ok()?;
    if canon.is_file() && canon.starts_with(root) {
        Some(canon)
    } else {
        None
    }
}

fn read_rule(root: &Path, path: &Path) -> Result<Rule, String> {
    let source = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let text = fs::read_to_string(path).map_err(|e| format!("{source}: {e}"))?;
    let (header, body) =
        frontmatter::parse::<Header>(&text).map_err(|e| format!("{source}: {e}"))?;
    let globs = frontmatter::names(header.paths.as_ref());
    if globs.is_empty() {
        return Err(format!("{source}: no paths globs"));
    }
    let mut builder = GlobSetBuilder::new();
    for glob in &globs {
        let glob = GlobBuilder::new(glob)
            .literal_separator(true)
            .build()
            .map_err(|e| format!("{source}: {e}"))?;
        builder.add(glob);
    }
    let set = builder.build().map_err(|e| format!("{source}: {e}"))?;
    let body = body.trim();
    if body.is_empty() {
        return Err(format!("{source}: empty body"));
    }
    Ok(Rule {
        globs,
        body: frame(&source, body),
        source,
        set,
    })
}

/// The model sees a project rule, never a block that looks like the system
/// prompt. The body is untrusted repository text.
fn frame(source: &str, body: &str) -> String {
    let source = source.replace(['"', '\n', '\r', '<', '>'], "_");
    let body = cox_sanitize::sanitize(body);
    format!(
        "<project_rule source=\"{source}\">\n{}\n</project_rule>\n",
        body.trim_end()
    )
}

impl Rule {
    fn matches(&self, path: &str) -> bool {
        let owned = normalize(path);
        let mut rest = owned.as_str();
        loop {
            if !rest.is_empty() && self.set.is_match(rest) {
                return true;
            }
            let Some((_, tail)) = rest.split_once('/') else {
                return false;
            };
            if tail.is_empty() {
                return false;
            }
            rest = tail;
        }
    }
}

fn normalize(path: &str) -> String {
    let slash = path.replace('\\', "/");
    let mut rest = slash.as_str();
    while let Some(stripped) = rest.strip_prefix("./") {
        rest = stripped;
    }
    rest.trim_start_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_matches_only_its_globs() {
        let dir = tempfile::tempdir().expect("temp");
        let cox = dir.path().join(".cox/rules");
        let claude = dir.path().join(".claude/rules");
        fs::create_dir_all(&cox).expect("cox rules");
        fs::create_dir_all(&claude).expect("claude rules");
        fs::write(
            cox.join("rust.md"),
            "---\npaths:\n  - src/**/*.rs\n---\nUse rustfmt.\n\u{1b}[31mred\n",
        )
        .expect("rust rule");
        fs::write(
            claude.join("docs.md"),
            "---\npaths: [\"docs/**/*.md\"]\n---\nDocs style.\n",
        )
        .expect("docs rule");
        fs::write(cox.join("broken.md"), "---\npaths: [\n").expect("broken");
        fs::write(cox.join("note.txt"), "not a rule").expect("ignored");

        let loaded = load_rules(&[dir.path().to_path_buf()]);
        assert!(
            loaded.notices.iter().any(|n| n.contains("broken.md")),
            "{:?}",
            loaded.notices
        );
        assert_eq!(loaded.rules.len(), 2, "{:?}", loaded.notices);
        let rust = rules_for(
            &["src/foo/lib.rs".into(), "README.md".into()],
            &loaded.rules,
        );
        assert_eq!(rust.len(), 1);
        assert_eq!(rust[0].source, ".cox/rules/rust.md");
        assert!(rust[0].body.contains("Use rustfmt."));
        assert!(rust[0].body.starts_with("<project_rule "));
        assert!(!rust[0].body.contains('\u{1b}'), "escapes are stripped");
        assert!(rules_for(&["README.md".into()], &loaded.rules).is_empty());
        assert_eq!(rules_for(&["docs/guide.md".into()], &loaded.rules).len(), 1);
        let abs = format!("{}/src/foo/main.rs", dir.path().display());
        assert_eq!(rules_for(&[abs], &loaded.rules).len(), 1);
        let both = rules_for(
            &["src/foo/lib.rs".into(), "docs/guide.md".into()],
            &loaded.rules,
        );
        assert_eq!(both.len(), 2);
    }
}
