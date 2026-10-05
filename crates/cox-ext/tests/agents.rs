// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T7.3: agent definitions parse and restrict a tool list; a child never
//! gains a tool by naming one the parent lacks.

use std::path::{Path, PathBuf};

use cox_ext::agents::{discover, tier_for};
use cox_protocol::types::Tier;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agents")
}

#[test]
fn agents_fixture_parses_and_maps_its_model() {
    let found = discover(&[fixtures()]);
    assert!(found.notices.is_empty(), "{:?}", found.notices);
    // Embedded explore/shell first, the fixture dir's reviewer appended.
    assert_eq!(
        found
            .agents
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        ["explore", "shell", "reviewer"]
    );
    let def = &found.agents[2];
    assert_eq!(def.name, "reviewer");
    assert_eq!(def.description, "Reviews diffs for correctness.");
    assert_eq!(def.tools, ["read", "grep", "glob"]);
    assert_eq!(tier_for(def.model.as_deref()), Some(Tier::Think));
    assert!(def.body.starts_with("You are a careful reviewer."));
}

#[test]
fn agents_restrict_keeps_only_listed_tools_the_parent_has() {
    let found = discover(&[fixtures()]);
    let def = found
        .agents
        .iter()
        .find(|a| a.name == "reviewer")
        .expect("reviewer");
    let parent: Vec<(String, u8)> = ["read", "edit", "grep", "bash"]
        .iter()
        .enumerate()
        .map(|(i, n)| (n.to_string(), i as u8))
        .collect();
    // `glob` is listed but absent from the parent: not conjured up.
    assert_eq!(def.restrict(&parent), [0, 2]);
}

#[test]
fn agents_missing_fields_are_skipped_with_a_notice() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.md"), "---\nname: a\n---\nbody\n").unwrap();
    std::fs::write(dir.path().join("b.md"), "no frontmatter\n").unwrap();
    let found = discover(&[dir.path().to_path_buf()]);
    // The two broken files are skipped; the embedded presets still ship.
    assert_eq!(
        found
            .agents
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        ["explore", "shell"]
    );
    assert_eq!(found.notices.len(), 2, "{:?}", found.notices);
}
