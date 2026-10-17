// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T66.5: harness entries apply with a backup, roll back byte for byte, are
//! fingerprinted by content, and a local edit stays inside its session.

// why: nextest filters on the test name, so the module name lets
// `nextest run -p cox-ext harness` find these.
mod harness {
    use std::fs;
    use std::path::Path;

    use cox_ext::harness::{Action, Edit, HarnessError, Kind, Scope};
    use tempfile::TempDir;

    fn edit(action: Action, kind: Kind, id: &str, content: Option<&str>) -> Edit {
        Edit {
            action,
            kind,
            id: id.to_string(),
            content: content.map(str::to_string),
        }
    }

    fn local(dir: &Path) -> Scope {
        Scope::Local {
            root: dir.to_path_buf(),
        }
    }

    fn global(dir: &Path) -> Scope {
        Scope::Global {
            home: dir.to_path_buf(),
            memory_dir: dir.join("projects").join("p").join("memory"),
        }
    }

    const FACT: &str =
        "---\nname: tabs\ndescription: tabs not spaces\ntype: feedback\n---\nUse tabs.";

    #[test]
    fn rollback_restores_the_bytes_and_deletes_what_the_event_created() {
        let tmp = TempDir::new().unwrap();
        let scope = global(tmp.path());
        scope
            .apply(
                "e1",
                &[
                    edit(Action::Create, Kind::PromptNote, "style", Some("v1")),
                    edit(Action::Create, Kind::Subagent, "scout", Some("agent v1")),
                ],
            )
            .unwrap();
        let untouched = scope.fingerprint().unwrap();

        let applied = scope
            .apply(
                "e2",
                &[
                    edit(Action::Update, Kind::PromptNote, "style", Some("v2")),
                    edit(Action::Delete, Kind::Subagent, "scout", None),
                    edit(Action::Create, Kind::Skill, "lint", Some("skill")),
                    edit(Action::Create, Kind::Memory, "tabs", Some(FACT)),
                ],
            )
            .unwrap();
        assert_eq!(applied.before, untouched);
        assert_ne!(applied.after, untouched);
        assert_eq!(
            fs::read_to_string(tmp.path().join("notes/style.md")).unwrap(),
            "v2"
        );
        assert!(tmp.path().join("skills/lint/SKILL.md").is_file());
        let index = fs::read_to_string(
            tmp.path()
                .join("projects/p/memory")
                .join(cox_ext::memory::INDEX_NAME),
        )
        .unwrap();
        assert!(index.contains("tabs"));

        scope.rollback("e2").unwrap();

        assert_eq!(
            fs::read_to_string(tmp.path().join("notes/style.md")).unwrap(),
            "v1"
        );
        assert_eq!(
            fs::read_to_string(tmp.path().join("agents/scout.md")).unwrap(),
            "agent v1"
        );
        assert!(!tmp.path().join("skills/lint").exists());
        assert!(!tmp.path().join("projects/p/memory/tabs.md").exists());
        assert_eq!(scope.fingerprint().unwrap(), untouched);
        assert!(matches!(
            scope.rollback("e2"),
            Err(HarnessError::NoBackup(_))
        ));
    }

    #[test]
    fn an_edit_records_the_hash_of_each_side() {
        let tmp = TempDir::new().unwrap();
        let scope = local(tmp.path());
        let created = scope
            .apply(
                "e1",
                &[edit(Action::Create, Kind::PromptNote, "a", Some("x"))],
            )
            .unwrap();
        assert_eq!(created.edits[0].before_sha, None);
        assert!(created.edits[0].after_sha.is_some());
        let removed = scope
            .apply("e2", &[edit(Action::Delete, Kind::PromptNote, "a", None)])
            .unwrap();
        assert_eq!(removed.edits[0].before_sha, created.edits[0].after_sha);
        assert_eq!(removed.edits[0].after_sha, None);
    }

    #[test]
    fn an_id_with_a_slash_or_dots_is_refused_and_nothing_is_written() {
        let tmp = TempDir::new().unwrap();
        let scope = local(tmp.path());
        for bad in ["a/b", "..", "../x", "", "A", "a.b", &"a".repeat(65)] {
            let err = scope
                .apply(
                    "e1",
                    &[edit(Action::Create, Kind::PromptNote, bad, Some("x"))],
                )
                .unwrap_err();
            assert!(matches!(err, HarnessError::InvalidId(_)), "{bad:?}: {err}");
        }
        assert!(matches!(
            scope.apply(
                "../e",
                &[edit(Action::Create, Kind::PromptNote, "a", Some("x"))]
            ),
            Err(HarnessError::InvalidId(_))
        ));
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_refused_edit_leaves_the_earlier_edits_of_its_batch_unwritten() {
        let tmp = TempDir::new().unwrap();
        let scope = local(tmp.path());
        let err = scope
            .apply(
                "e1",
                &[
                    edit(Action::Create, Kind::PromptNote, "a", Some("x")),
                    edit(Action::Update, Kind::PromptNote, "missing", Some("y")),
                ],
            )
            .unwrap_err();
        assert!(matches!(err, HarnessError::Refused { .. }));
        assert!(scope.load().unwrap().is_empty());
        assert!(!tmp.path().join("harness-backup").exists());
    }

    #[test]
    fn create_over_an_existing_entry_and_missing_content_are_refused() {
        let tmp = TempDir::new().unwrap();
        let scope = local(tmp.path());
        scope
            .apply("e1", &[edit(Action::Create, Kind::Skill, "a", Some("x"))])
            .unwrap();
        for bad in [
            edit(Action::Create, Kind::Skill, "a", Some("y")),
            edit(Action::Update, Kind::Skill, "a", None),
            edit(Action::Delete, Kind::Skill, "a", Some("y")),
        ] {
            assert!(matches!(
                scope.apply("e2", &[bad]),
                Err(HarnessError::Refused { .. })
            ));
        }
        assert!(matches!(
            scope.apply(
                "e2",
                &[
                    edit(Action::Update, Kind::Skill, "a", Some("y")),
                    edit(Action::Delete, Kind::Skill, "a", None),
                ]
            ),
            Err(HarnessError::Duplicate { .. })
        ));
        assert!(matches!(scope.apply("e2", &[]), Err(HarnessError::Empty)));
    }

    #[test]
    fn an_event_id_is_used_once() {
        let tmp = TempDir::new().unwrap();
        let scope = local(tmp.path());
        scope
            .apply(
                "e1",
                &[edit(Action::Create, Kind::PromptNote, "a", Some("x"))],
            )
            .unwrap();
        assert!(matches!(
            scope.apply(
                "e1",
                &[edit(Action::Update, Kind::PromptNote, "a", Some("y"))]
            ),
            Err(HarnessError::EventExists(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn an_entry_that_is_a_symlink_is_refused() {
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("secret.md");
        fs::write(&target, "keep").unwrap();
        let scope = local(tmp.path());
        fs::create_dir_all(tmp.path().join("harness/prompt-note")).unwrap();
        std::os::unix::fs::symlink(&target, tmp.path().join("harness/prompt-note/a.md")).unwrap();

        let err = scope
            .apply(
                "e1",
                &[edit(Action::Update, Kind::PromptNote, "a", Some("x"))],
            )
            .unwrap_err();
        assert!(matches!(err, HarnessError::Refused { .. }));
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
    }

    #[test]
    fn the_same_store_gives_the_same_fingerprint() {
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        let edits = [
            edit(Action::Create, Kind::PromptNote, "n", Some("note")),
            edit(Action::Create, Kind::Subagent, "s", Some("agent")),
        ];
        let first = global(a.path()).apply("e1", &edits).unwrap();
        let reversed: Vec<Edit> = edits.iter().rev().cloned().collect();
        let second = global(b.path()).apply("e9", &reversed).unwrap();
        assert_eq!(first.after, second.after);
        assert_eq!(first.after, global(a.path()).fingerprint().unwrap());
    }

    #[test]
    fn a_content_change_or_a_rename_changes_the_fingerprint() {
        let tmp = TempDir::new().unwrap();
        let scope = local(tmp.path());
        let one = scope
            .apply(
                "e1",
                &[edit(Action::Create, Kind::PromptNote, "a", Some("x"))],
            )
            .unwrap();
        let two = scope
            .apply(
                "e2",
                &[edit(Action::Update, Kind::PromptNote, "a", Some("y"))],
            )
            .unwrap();
        assert_ne!(one.after, two.after);
        scope.rollback("e2").unwrap();
        let other = TempDir::new().unwrap();
        let renamed = local(other.path())
            .apply(
                "e1",
                &[edit(Action::Create, Kind::PromptNote, "b", Some("x"))],
            )
            .unwrap();
        assert_ne!(one.after, renamed.after);
    }

    #[test]
    fn a_local_edit_is_invisible_to_another_sessions_load() {
        let s1 = TempDir::new().unwrap();
        let s2 = TempDir::new().unwrap();
        let empty = local(s2.path()).fingerprint().unwrap();

        local(s1.path())
            .apply("e1", &[edit(Action::Create, Kind::Skill, "a", Some("x"))])
            .unwrap();

        assert!(local(s2.path()).load().unwrap().is_empty());
        assert_eq!(local(s2.path()).fingerprint().unwrap(), empty);
        assert_eq!(local(s1.path()).load().unwrap().len(), 1);
    }
}
