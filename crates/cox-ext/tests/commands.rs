// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T7.3: command files expand arguments, shell and file inclusions through
//! the caller's `Includes`; broken headers are skipped with a notice.

use std::path::{Path, PathBuf};

use cox_ext::commands::{Includes, command_dirs, discover, expand};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/commands")
}

/// Records what expansion asked for; `git` answers, everything else fails.
#[derive(Default)]
struct Stub {
    shells: Vec<String>,
    files: Vec<String>,
}

impl Includes for Stub {
    fn shell(&mut self, command: &str) -> Result<String, String> {
        self.shells.push(command.to_string());
        if command.starts_with("git ") {
            Ok("main\n".into())
        } else {
            Err("denied".into())
        }
    }
    fn file(&mut self, path: &str) -> Result<String, String> {
        self.files.push(path.to_string());
        if path == "STYLE.md" {
            Ok("Two spaces.\n".into())
        } else {
            Err("not found".into())
        }
    }
}

#[test]
fn commands_fixture_parses_frontmatter_and_plain_bodies() {
    let found = discover(&[fixtures()]);
    assert!(found.notices.is_empty(), "{:?}", found.notices);
    let names: Vec<&str> = found.commands.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["plain", "review"]);
    let review = &found.commands[1];
    assert_eq!(
        review.description.as_deref(),
        Some("Review a file against the style guide")
    );
    assert_eq!(review.allowed_tools, ["read", "grep"]);
    assert_eq!(review.model.as_deref(), Some("haiku"));
    assert_eq!(review.argument_hint.as_deref(), Some("<path> [focus]"));
    assert_eq!(found.commands[0].body, "Just say hi to $ARGUMENTS.");
}

#[test]
fn commands_expand_arguments_shell_and_file_inclusions() {
    let found = discover(&[fixtures()]);
    let review = found.commands.iter().find(|c| c.name == "review").unwrap();
    let mut stub = Stub::default();
    let (text, notices) = expand(review, "src/lib.rs naming", &mut stub);
    assert!(notices.is_empty(), "{notices:?}");
    assert_eq!(
        text,
        "Review src/lib.rs focusing on naming. Full args: src/lib.rs naming.\n\nBranch: main\nGuide:\nTwo spaces.\nContact me@example.com, not $HOME."
    );
    assert_eq!(stub.shells, ["git branch --show-current"]);
    assert_eq!(stub.files, ["STYLE.md"]);
}

#[test]
fn commands_failed_inclusions_stay_verbatim_with_a_notice() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("danger.md"),
        "Run !`rm -rf /` then read @missing.md and $3.\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("broken.md"),
        "---\ndescription: [\n---\nx\n",
    )
    .unwrap();
    let found = discover(&[dir.path().to_path_buf()]);
    assert_eq!(found.commands.len(), 1);
    assert_eq!(found.notices.len(), 1);
    assert!(found.notices[0].contains("broken.md skipped"));
    let mut stub = Stub::default();
    let (text, notices) = expand(&found.commands[0], "a b", &mut stub);
    assert_eq!(text, "Run !`rm -rf /` then read @missing.md and .");
    assert_eq!(notices, ["command `danger`: !`rm -rf /`: denied"]);
}

#[test]
fn commands_project_dirs_override_home() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    for (root, body) in [(home.path(), "home"), (project.path(), "project")] {
        let d = root.join(".claude/commands");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("dup.md"), body).unwrap();
    }
    let dirs = command_dirs(
        None,
        Some(&home.path().join(".claude")),
        Some(project.path()),
    );
    let found = discover(&dirs);
    assert_eq!(found.commands.len(), 1);
    assert_eq!(found.commands[0].body, "project");
}
