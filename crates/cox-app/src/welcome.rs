//! An empty session's welcome hero (Figma frame 22-empty-session, T37.49):
//! one line about the folder — the project kind and size from its manifest
//! and the instruction files a session there loads — and three suggestions
//! that fill the composer. Here, not in Swift, so the app reads no manifest
//! (DS§1); the instruction files are `cox_ext::instructions::load`'s own, so
//! the line names what the model really gets (T7.8). Reads files only: no
//! `cargo` or `git` runs at open.

use std::path::Path;

use crate::app::App;

/// The hero's facts about one folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Welcome {
    /// `Rust workspace · 31 crates · AGENTS.md loaded`; empty when nothing is known.
    pub summary: String,
    pub suggestions: Vec<Suggestion>,
}

/// One suggestion card: its title, the line under it, and the prompt a click drafts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub title: String,
    pub detail: String,
    pub prompt: String,
}

/// What the folder's manifest says it is; the first manifest found wins.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Cargo {
        members: Option<usize>,
        nextest: bool,
    },
    Node {
        packages: Option<usize>,
    },
    Go,
    Python,
}

impl App {
    /// The welcome for a session in `cwd`, under this home's instruction
    /// budget; a config that does not load uses the default budget.
    pub fn welcome(&self, cwd: &Path) -> Welcome {
        let budget = self
            .config(cwd)
            .map(|c| c.context.instruction_budget_tokens)
            .unwrap_or_else(|_| {
                cox_protocol::Config::default()
                    .context
                    .instruction_budget_tokens
            });
        let claude = cox_config::load::home_dir().join(".claude");
        let roots = cox_session::instruction_roots(&self.home, &claude, cwd);
        let loaded = cox_ext::instructions::load(&roots, budget).files;
        facts(cwd, &loaded, roots.git_root.is_some())
    }
}

/// The welcome for `cwd` given the instruction files a session loads there
/// (as `instructions::load` names them) and whether it is in a git repository.
pub fn facts(cwd: &Path, instructions: &[String], in_git: bool) -> Welcome {
    let kind = kind(cwd);
    let mut parts: Vec<String> = kind.iter().flat_map(describe).collect();
    let mut names: Vec<&str> = instructions.iter().map(|f| file_name(f)).collect();
    names.dedup();
    if !names.is_empty() {
        parts.push(format!("{} loaded", names.join(", ")));
    }
    let project = cwd
        .file_name()
        .map_or_else(|| "this project".into(), |n| n.to_string_lossy());
    let test = kind.as_ref().map(test_command);
    let mut suggestions = vec![
        Suggestion {
            title: "Explain the architecture".into(),
            detail: format!("How do the parts of {project} fit together?"),
            prompt: format!(
                "Explain the architecture: how do the parts of {project} fit together?"
            ),
        },
        Suggestion {
            title: "Find and fix a failing test".into(),
            detail: test.map_or_else(
                || "Run the tests and fix the first failure".into(),
                |t| format!("Run {t} and fix the first failure"),
            ),
            prompt: test.map_or_else(
                || "Run the tests and fix the first failure.".into(),
                |t| format!("Run {t} and fix the first failure."),
            ),
        },
    ];
    if in_git {
        suggestions.push(Suggestion {
            title: "Review my uncommitted diff".into(),
            detail: "Check git diff for bugs before I commit".into(),
            prompt: "Review my uncommitted diff: check git diff for bugs before I commit.".into(),
        });
    }
    Welcome {
        summary: parts.join(" · "),
        suggestions,
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn describe(kind: &Kind) -> Vec<String> {
    let count = |n: &Option<usize>, one: &str, many: &str| {
        n.map(|n| format!("{n} {}", if n == 1 { one } else { many }))
    };
    match kind {
        Kind::Cargo {
            members: Some(n), ..
        } => {
            vec![
                "Rust workspace".into(),
                count(&Some(*n), "crate", "crates").unwrap_or_default(),
            ]
        }
        Kind::Cargo { members: None, .. } => vec!["Rust crate".into()],
        Kind::Node { packages: Some(n) } => {
            vec![
                "Node workspace".into(),
                count(&Some(*n), "package", "packages").unwrap_or_default(),
            ]
        }
        Kind::Node { packages: None } => vec!["Node project".into()],
        Kind::Go => vec!["Go module".into()],
        Kind::Python => vec!["Python project".into()],
    }
}

fn test_command(kind: &Kind) -> &'static str {
    match kind {
        Kind::Cargo { nextest: true, .. } => "cargo nextest",
        Kind::Cargo { nextest: false, .. } => "cargo test",
        Kind::Node { .. } => "npm test",
        Kind::Go => "go test ./...",
        Kind::Python => "pytest",
    }
}

fn kind(cwd: &Path) -> Option<Kind> {
    if let Ok(text) = std::fs::read_to_string(cwd.join("Cargo.toml")) {
        let doc = text.parse::<toml_edit::DocumentMut>().ok();
        let workspace = doc.as_ref().and_then(|d| d.get("workspace"));
        let list = |key: &str| -> Vec<String> {
            workspace
                .and_then(|w| w.get(key))
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        };
        let members =
            workspace.map(|_| expand(cwd, &list("members"), &list("exclude"), "Cargo.toml"));
        let nextest = cwd.join(".config/nextest.toml").is_file();
        return Some(Kind::Cargo { members, nextest });
    }
    if let Ok(text) = std::fs::read_to_string(cwd.join("package.json")) {
        let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        let globs = json
            .get("workspaces")
            .map(|w| w.get("packages").unwrap_or(w));
        let packages = globs.and_then(|g| g.as_array()).map(|a| {
            let globs: Vec<String> = a
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            expand(cwd, &globs, &[], "package.json")
        });
        return Some(Kind::Node { packages });
    }
    if cwd.join("go.mod").is_file() {
        return Some(Kind::Go);
    }
    ["pyproject.toml", "setup.py"]
        .iter()
        .any(|f| cwd.join(f).is_file())
        .then_some(Kind::Python)
}

/// How many directories the member `globs` name that hold a `manifest`, less
/// `exclude`. A glob is a path or a path ending in `/*`, as workspaces write
/// them; a glob with any other wildcard counts nothing.
fn expand(root: &Path, globs: &[String], exclude: &[String], manifest: &str) -> usize {
    let mut dirs = std::collections::BTreeSet::new();
    for glob in globs {
        if let Some(parent) = glob.strip_suffix("/*") {
            let entries = std::fs::read_dir(root.join(parent))
                .into_iter()
                .flatten()
                .flatten();
            dirs.extend(
                entries
                    .map(|e| e.path())
                    .filter(|p| p.join(manifest).is_file()),
            );
        } else if !glob.contains(['*', '?', '[']) && root.join(glob).join(manifest).is_file() {
            dirs.insert(root.join(glob));
        }
    }
    exclude.iter().for_each(|e| {
        dirs.remove(&root.join(e));
    });
    dirs.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_cargo_workspace_counts_its_member_crates_and_names_its_instructions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\", \"tool\"]\nexclude = [\"crates/old\"]\n",
        );
        for c in ["a", "b", "old"] {
            write(root, &format!("crates/{c}/Cargo.toml"), "[package]\n");
        }
        write(root, "crates/notes/README.md", "");
        write(root, "tool/Cargo.toml", "[package]\n");
        write(root, ".config/nextest.toml", "");
        let welcome = facts(
            root,
            &["AGENTS.md".into(), "/home/me/.claude/CLAUDE.md".into()],
            true,
        );
        assert_eq!(
            welcome.summary,
            "Rust workspace · 3 crates · AGENTS.md, CLAUDE.md loaded"
        );
        assert_eq!(welcome.suggestions.len(), 3);
        assert_eq!(
            welcome.suggestions[1].prompt,
            "Run cargo nextest and fix the first failure."
        );
    }

    #[test]
    fn a_single_crate_runs_cargo_test_and_outside_git_has_no_diff_suggestion() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Cargo.toml", "[package]\nname = \"one\"\n");
        let welcome = facts(dir.path(), &[], false);
        assert_eq!(welcome.summary, "Rust crate");
        assert_eq!(
            welcome.suggestions[1].detail,
            "Run cargo test and fix the first failure"
        );
        assert_eq!(
            welcome.suggestions.len(),
            2,
            "no diff to review outside git"
        );
    }

    #[test]
    fn a_node_workspace_counts_its_packages() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "package.json",
            r#"{"workspaces": {"packages": ["packages/*"]}}"#,
        );
        write(root, "packages/web/package.json", "{}");
        write(root, "packages/api/package.json", "{}");
        let welcome = facts(root, &["AGENTS.md".into()], true);
        assert_eq!(
            welcome.summary,
            "Node workspace · 2 packages · AGENTS.md loaded"
        );
        assert_eq!(
            welcome.suggestions[1].prompt,
            "Run npm test and fix the first failure."
        );
    }

    #[test]
    fn an_empty_folder_shows_only_what_it_knows() {
        let dir = tempfile::tempdir().unwrap();
        let welcome = facts(dir.path(), &[], false);
        assert_eq!(welcome.summary, "");
        assert_eq!(
            welcome.suggestions[1].prompt,
            "Run the tests and fix the first failure."
        );
        let name = dir
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(welcome.suggestions[0].detail.contains(&name));
    }
}
