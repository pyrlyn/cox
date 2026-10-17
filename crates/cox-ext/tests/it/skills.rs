// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T7.2: skills from a vendored `anthropics/skills` sample plus cox's own —
//! the index carries names and descriptions only; the body arrives on
//! invoke, with `allowed-tools` alongside for the engine.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use cox_ext::skills::{Discovered, SkillTool, discover, index, merge, parse_file, skill_dirs};
use cox_protocol::errors::{StoreError, ToolError};
use cox_protocol::ids::{ArchiveId, CallId, SessionId};
use cox_protocol::traits::{Archive, ArchivePut, Tool, ToolCx};
use cox_protocol::types::{LinuxBackend, SandboxMode, SandboxPolicy};
use serde_json::json;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skills")
}

struct NoArchive;

#[async_trait]
impl Archive for NoArchive {
    async fn put(&self, _put: ArchivePut) -> Result<ArchiveId, StoreError> {
        Ok(ArchiveId::new())
    }
    async fn get(&self, _id: &ArchiveId) -> Result<Vec<u8>, StoreError> {
        Ok(Vec::new())
    }
}

fn cx() -> ToolCx {
    ToolCx {
        roots: vec![PathBuf::from(".")],
        writable_roots: vec![PathBuf::from(".")],
        cwd: PathBuf::from("."),
        sandbox: SandboxPolicy {
            mode: SandboxMode::ReadOnly,
            network: false,
            writable: Vec::new(),
            readonly_in_workspace: Vec::new(),
            linux_backend: LinuxBackend::Auto,
        },
        archive: Arc::new(NoArchive),
        cancel: tokio_util::sync::CancellationToken::new(),
        output: tokio::sync::mpsc::channel(1).0,
        session: SessionId::new(),
        call: CallId::new(),
        agent: None,
        preset: None,
        relay: None,
    }
}

#[test]
fn skills_index_lists_names_and_descriptions_without_bodies() {
    let found = discover(&[fixtures()]);
    assert!(found.notices.is_empty(), "{:?}", found.notices);
    let names: Vec<&str> = found.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["greeting", "skill-creator"]);
    let text = index(&found.skills);
    assert!(text.starts_with("# Skills\n"), "{text}");
    assert!(
        text.contains("- skill-creator: Create new skills"),
        "{text}"
    );
    assert!(
        text.contains("- greeting: Greet the user in their language"),
        "{text}"
    );
    // Two skills, two index lines, no body content.
    assert_eq!(text.lines().filter(|l| l.starts_with("- ")).count(), 2);
    assert!(!text.contains("Say hello in the language"), "{text}");
    assert!(index(&[]).is_empty());
}

#[tokio::test]
async fn skills_invoke_returns_the_body_and_allowed_tools() {
    let found = discover(&[fixtures()]);
    let tool = SkillTool::new(found.skills.clone());
    assert!(tool.spec().deferred);
    let out = tool
        .call(json!({ "name": "greeting" }), &cx())
        .await
        .unwrap();
    let dir = fixtures().join("greeting").display().to_string();
    assert!(
        out.text.starts_with(&format!(
            "# Skill: greeting\n\nBase directory: {dir}\n\n# Greeting\n"
        )),
        "{}",
        out.text
    );
    assert!(out.text.contains("Say hello in the language"));
    assert_eq!(
        out.structured.unwrap()["allowed_tools"],
        json!(["read", "grep"])
    );

    let sample = tool
        .call(json!({ "name": "skill-creator" }), &cx())
        .await
        .unwrap();
    let vendored = std::fs::read_to_string(fixtures().join("skill-creator/SKILL.md")).unwrap();
    // The whole vendored body is there, not a summary.
    assert!(
        sample.text.len() > vendored.len() / 2,
        "{}",
        sample.text.len()
    );
    assert!(matches!(
        tool.call(json!({ "name": "nope" }), &cx()).await,
        Err(ToolError::NotFound)
    ));
}

#[test]
fn skills_vendored_sample_parses_its_frontmatter() {
    let found = discover(&[fixtures()]);
    let sample = found
        .skills
        .iter()
        .find(|s| s.name == "skill-creator")
        .unwrap();
    assert!(sample.description.len() > 40);
    assert!(sample.allowed_tools.is_empty());
    let own = found.skills.iter().find(|s| s.name == "greeting").unwrap();
    assert_eq!(own.license.as_deref(), Some("MIT"));
    assert_eq!(own.metadata["author"], "cox");
    assert_eq!(own.metadata["version"], "1");
    assert!(
        own.compatibility
            .as_deref()
            .unwrap()
            .starts_with("Needs nothing")
    );
}

#[test]
fn skills_malformed_or_misnamed_are_skipped_with_a_notice() {
    let dir = tempfile::tempdir().unwrap();
    let mk = |name: &str, text: &str| {
        let d = dir.path().join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("SKILL.md"), text).unwrap();
    };
    mk("no-front", "# Just markdown\n");
    mk("bad-yaml", "---\nname: [unclosed\n---\nbody\n");
    mk("mismatch", "---\nname: other\ndescription: d\n---\nbody\n");
    mk("no-desc", "---\nname: no-desc\n---\nbody\n");
    mk("Upper", "---\nname: Upper\ndescription: d\n---\nbody\n");
    mk("good", "---\nname: good\ndescription: fine\n---\nbody\n");
    let found = discover(&[dir.path().to_path_buf()]);
    let names: Vec<&str> = found.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["good"]);
    assert_eq!(found.notices.len(), 5, "{:?}", found.notices);
    assert!(
        found
            .notices
            .iter()
            .all(|n| n.starts_with("skill ") && n.contains("skipped: "))
    );
}

#[test]
fn skills_later_directories_override_earlier_same_names() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    for (root, desc) in [(home.path(), "from home"), (project.path(), "from project")] {
        let d = root.join(".claude/skills/dup");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("SKILL.md"),
            format!("---\nname: dup\ndescription: {desc}\n---\nbody\n"),
        )
        .unwrap();
    }
    let dirs = skill_dirs(
        None,
        Some(&home.path().join(".claude")),
        Some(project.path()),
    );
    assert_eq!(dirs.len(), 3);
    let found = discover(&dirs);
    assert_eq!(found.skills.len(), 1);
    assert_eq!(found.skills[0].description, "from project");
}

/// T33.46: a plugin's skill is read with `discover`'s own parser, and a
/// local skill of the same name keeps its place.
#[test]
fn skills_plugin_skill_loses_to_a_local_one_with_a_notice() {
    let mut local = discover(&[fixtures()]);
    let greeting = parse_file(&fixtures().join("greeting/SKILL.md")).unwrap();
    let creator = parse_file(&fixtures().join("skill-creator/SKILL.md")).unwrap();
    // Only the local `greeting`: the plugin's `skill-creator` joins, its
    // `greeting` does not.
    let mut some = Discovered {
        skills: local
            .skills
            .iter()
            .filter(|s| s.name == "greeting")
            .cloned()
            .collect(),
        notices: Vec::new(),
    };
    merge(&mut some, vec![greeting.clone(), creator], "kit");
    let names: Vec<&str> = some.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["greeting", "skill-creator"]);
    assert_eq!(
        some.notices,
        ["plugin kit: skill greeting skipped; a skill with that name is already loaded"]
    );
    let before = local.skills.clone();
    merge(&mut local, vec![greeting], "kit");
    assert_eq!(local.skills, before);
}

/// T33.46: a file outside a skill dir still has to be named for its folder.
#[test]
fn skills_parse_file_applies_the_name_rule() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("other/SKILL.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "---\nname: greeting\ndescription: x\n---\nbody\n").unwrap();
    let err = parse_file(&path).unwrap_err();
    assert!(err.contains("does not match its directory"), "{err}");
}

#[tokio::test]
async fn skills_invoke_names_the_skill_folder() {
    let tool = SkillTool::new(discover(&[fixtures()]).skills);
    let out = tool
        .call(json!({ "name": "skill-creator" }), &cx())
        .await
        .unwrap();
    let dir = fixtures().join("skill-creator");
    assert_eq!(
        out.structured.unwrap()["dir"],
        json!(dir.display().to_string())
    );
    assert!(dir.join("SKILL.md").is_file());
}
