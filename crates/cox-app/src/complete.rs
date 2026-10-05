// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Composer completion (DT§5.3): `/` offers the shared built-in table plus
//! the project's command files, `@` offers the files the `glob` tool would
//! find. The sources are the TUI's — `cox_protocol::commands::COMMANDS`,
//! `cox_ext::commands::discover`, `cox_search::glob::workspace_files` —
//! ranked by the same nucleo scorer, so both surfaces offer the same rows.
//! Which token at the caret asks for rows and how a picked row lands in the
//! draft are decided here too (T58.4.16), so every client splices alike.

use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

use cox_protocol::commands::COMMANDS;
use cox_search::glob::{Candidate, rank_by_query, workspace_files};
use serde::{Deserialize, Serialize};

use crate::palette::{PaletteHit, PaletteItem, PaletteKind, rank};

/// One completion row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Completion {
    /// What replaces the typed token (`/compact`, `@src/lib.rs`).
    pub insert: String,
    /// Usage or path, for the row's second line.
    pub detail: String,
}

/// A session's completion sources, loaded once when it opens.
#[derive(Debug, Clone, Default)]
pub struct Completer {
    commands: Vec<Completion>,
    files: Vec<String>,
    /// The project's directory name, a file's detail in the palette.
    project: String,
}

impl Completer {
    /// Blocking (a directory walk and file reads): the caller runs it on
    /// the blocking pool. `home` is `COX_HOME`, `claude_home` `~/.claude`.
    /// A broken command file is skipped, never fatal.
    pub fn load(cwd: &Path, home: &Path, claude_home: &Path) -> Self {
        let root = cox_config::load::find_git_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
        let dirs = cox_ext::commands::command_dirs(Some(home), Some(claude_home), Some(&root));
        let mut commands: Vec<Completion> = COMMANDS
            .iter()
            .map(|(name, usage, _)| Completion {
                insert: format!("/{name}"),
                detail: (*usage).to_string(),
            })
            .collect();
        for c in cox_ext::commands::discover(&dirs).commands {
            if COMMANDS.iter().any(|(n, ..)| *n == c.name) {
                continue;
            }
            commands.push(Completion {
                insert: format!("/{}", c.name),
                detail: c.description.unwrap_or_default(),
            });
        }
        let project = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            commands,
            files: workspace_files(cwd),
            project,
        }
    }

    /// Appends `rows` after the built-ins and command files, skipping any
    /// already offered, so a plugin's command never shadows a row (T52.14).
    pub fn extend(&mut self, rows: impl IntoIterator<Item = Completion>) {
        for row in rows {
            if !self.commands.iter().any(|c| c.insert == row.insert) {
                self.commands.push(row);
            }
        }
    }

    /// Rows for the token being typed: `/que…` or `@que…`, best first,
    /// at most `limit`. Anything else completes nothing.
    pub fn complete(&self, token: &str, limit: usize) -> Vec<Completion> {
        let (sigil, query, rows) = if let Some(q) = token.strip_prefix('/') {
            ('/', q, self.commands.clone())
        } else if let Some(q) = token.strip_prefix('@') {
            let files = self.files.iter().map(|f| Completion {
                insert: format!("@{f}"),
                detail: f.clone(),
            });
            ('@', q, files.collect())
        } else {
            return Vec::new();
        };
        if query.is_empty() {
            return rows.into_iter().take(limit).collect();
        }
        let mut by_name: HashMap<String, Completion> = rows
            .into_iter()
            .map(|c| {
                (
                    c.insert
                        .strip_prefix(sigil)
                        .unwrap_or(&c.insert)
                        .to_string(),
                    c,
                )
            })
            .collect();
        let mut found: Vec<Candidate> = by_name
            .keys()
            .map(|name| Candidate {
                display: name.clone(),
                mtime: SystemTime::UNIX_EPOCH,
            })
            .collect();
        rank_by_query(&mut found, query);
        found
            .into_iter()
            .take(limit)
            .filter_map(|f| by_name.remove(&f.display))
            .collect()
    }

    /// The command palette's rows for `query` (T37.44.13): the caller's
    /// actions and sessions, then, once something is typed, this session's
    /// `/` commands and `@` files, ranked by [`rank`].
    pub fn palette(
        &self,
        query: &str,
        mut items: Vec<PaletteItem>,
        per_kind: usize,
    ) -> Vec<PaletteHit> {
        if !query.trim().is_empty() {
            items.extend(self.commands.iter().map(|c| PaletteItem {
                kind: PaletteKind::Command,
                id: c.insert.clone(),
                title: c.insert.clone(),
                detail: c.detail.clone(),
            }));
            items.extend(self.files.iter().map(|f| PaletteItem {
                kind: PaletteKind::File,
                id: format!("@{f}"),
                title: f.clone(),
                detail: self.project.clone(),
            }));
        }
        rank(query, items, per_kind)
    }
}

/// The word the caret ends when it asks for rows (T58.4.16). Offsets are
/// UTF-16 units, which both clients' strings index by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedToken {
    pub start: u32,
    pub end: u32,
    /// `@que…` or `/que…`, what [`Completer::complete`] takes.
    pub text: String,
}

/// A draft after a pick: its text and the caret, in UTF-16 units.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Splice {
    pub text: String,
    pub caret: u32,
}

/// The token at `caret` that asks for rows: an `@` file anywhere, a `/`
/// command only as the draft's first word. None inside a word, while text
/// is `selection`-ed, in shell mode, or when `caret` splits a character.
pub fn typed_token(text: &str, caret: u32, selection: bool, shell: bool) -> Option<TypedToken> {
    if shell || selection {
        return None;
    }
    let (head, tail) = text.split_at(byte_offset(text, caret)?);
    if tail.chars().next().is_some_and(|c| !c.is_whitespace()) {
        return None;
    }
    if head.chars().next_back()?.is_whitespace() {
        return None;
    }
    let start = head
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map(|(i, c)| i + c.len_utf8());
    let word = &head[start.unwrap_or(0)..];
    if !(word.starts_with('@') || (word.starts_with('/') && start.is_none())) {
        return None;
    }
    Some(TypedToken {
        start: utf16_len(&head[..start.unwrap_or(0)]),
        end: caret,
        text: word.to_string(),
    })
}

/// Puts `insert` in place of `token`, then one space (taking the one that
/// followed, if any), with the caret after it. An empty token at the end
/// of a word starts a new word, so an insert appended to the draft (the
/// palette's, T37.44.13) stands as a picked row would. `None` when the
/// token's offsets do not fall between the text's characters.
pub fn pick(text: &str, token: &TypedToken, insert: &str) -> Option<Splice> {
    let (start, end) = (
        byte_offset(text, token.start)?,
        byte_offset(text, token.end)?,
    );
    if start > end {
        return None;
    }
    let mut head = text[..start].to_string();
    if start == end && !head.is_empty() && !head.ends_with(' ') {
        head.push(' ');
    }
    head.push_str(insert);
    head.push(' ');
    let rest = &text[end..];
    let caret = utf16_len(&head);
    Some(Splice {
        text: head + rest.strip_prefix(' ').unwrap_or(rest),
        caret,
    })
}

/// The picked `@` files still in `text`, each once, in the order picked.
pub fn mentions(text: &str, picked: Vec<String>) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for m in picked {
        if m.starts_with('@') && text.contains(m.as_str()) && !kept.contains(&m) {
            kept.push(m);
        }
    }
    kept
}

/// The byte index `units` UTF-16 units into `text`, if it falls between
/// characters.
fn byte_offset(text: &str, units: u32) -> Option<usize> {
    let mut seen = 0u32;
    for (i, c) in text.char_indices() {
        if seen == units {
            return Some(i);
        }
        seen += u32::try_from(c.len_utf16()).ok()?;
    }
    (seen == units).then_some(text.len())
}

fn utf16_len(text: &str) -> u32 {
    u32::try_from(text.encode_utf16().count()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_offers_builtins_and_command_files_and_at_offers_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path().join("project");
        std::fs::create_dir_all(cwd.join("src")).expect("src");
        std::fs::write(cwd.join("src/lib.rs"), "").expect("lib");
        std::fs::create_dir_all(cwd.join(".cox/commands")).expect("commands");
        let deploy = "---\ndescription: ship it\n---\nDeploy $ARGUMENTS\n";
        std::fs::write(cwd.join(".cox/commands/deploy.md"), deploy).expect("deploy");
        let home = dir.path().join("cox-home");
        let c = Completer::load(&cwd, &home, &dir.path().join("claude"));

        let all = c.complete("/", 500);
        assert_eq!(
            all[0].insert,
            format!("/{}", COMMANDS[0].0),
            "built-ins first"
        );
        assert_eq!(all.len(), COMMANDS.len() + 1);
        let deploy = &c.complete("/depl", 3)[0];
        assert_eq!(
            (deploy.insert.as_str(), deploy.detail.as_str()),
            ("/deploy", "ship it")
        );
        assert_eq!(c.complete("/compact", 1)[0].insert, "/compact");
        assert_eq!(c.complete("@lib", 3)[0].insert, "@src/lib.rs");
        assert!(c.complete("plain", 3).is_empty());
    }

    #[test]
    fn palette_adds_commands_and_files_only_once_something_is_typed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path().join("project");
        std::fs::create_dir_all(cwd.join("docs")).expect("docs");
        std::fs::write(cwd.join("docs/review.md"), "").expect("review");
        let c = Completer::load(&cwd, &dir.path().join("h"), &dir.path().join("claude"));
        let action = PaletteItem {
            kind: PaletteKind::Action,
            id: "review".into(),
            title: "Review changes".into(),
            detail: "⌘⇧R".into(),
        };

        assert_eq!(c.palette("", vec![action.clone()], 5).len(), 1);
        let hits = c.palette("review", vec![action], 5);
        assert_eq!(hits[0].item.kind, PaletteKind::Action);
        let file = hits.iter().find(|h| h.item.kind == PaletteKind::File);
        let file = file.map(|h| (h.item.id.as_str(), h.item.detail.as_str()));
        assert_eq!(file, Some(("@docs/review.md", "project")));
    }

    fn end(text: &str) -> Option<String> {
        typed_token(text, utf16_len(text), false, false).map(|t| t.text)
    }

    #[test]
    fn a_slash_counts_only_as_the_first_word() {
        assert_eq!(end("/comp").as_deref(), Some("/comp"));
        assert_eq!(end("fix /comp"), None);
        assert_eq!(
            end(" /comp"),
            None,
            "a leading space makes it a second word"
        );
        assert_eq!(end("fix @src").as_deref(), Some("@src"));
        assert_eq!(end("fix\n@src").as_deref(), Some("@src"));
    }

    #[test]
    fn no_token_inside_a_word() {
        let text = "@src/lib.rs now";
        assert_eq!(typed_token(text, 4, false, false), None, "caret inside");
        let t = typed_token(text, 11, false, false);
        assert_eq!(
            t.map(|t| (t.start, t.end)),
            Some((0, 11)),
            "caret at its end"
        );
        assert_eq!(end("fix "), None, "caret after a space");
        assert_eq!(end(""), None);
        assert_eq!(typed_token("@a", 2, true, false), None, "with a selection");
        assert_eq!(typed_token("@a", 2, false, true), None, "in shell mode");
        assert_eq!(typed_token("@a", 9, false, false), None, "past the end");
    }

    #[test]
    fn a_pick_leaves_one_space_and_the_caret_after_it() {
        let token = typed_token("see @li now", 7, false, false).expect("token");
        let s = pick("see @li now", &token, "@src/lib.rs").expect("pick");
        assert_eq!((s.text.as_str(), s.caret), ("see @src/lib.rs now", 16));
        let token = typed_token("/co", 3, false, false).expect("token");
        let s = pick("/co", &token, "/compact").expect("pick");
        assert_eq!((s.text.as_str(), s.caret), ("/compact ", 9));
        let at_end = |text: &str| TypedToken {
            start: utf16_len(text),
            end: utf16_len(text),
            text: String::new(),
        };
        let appended = |text: &str| pick(text, &at_end(text), "@a").map(|s| s.text);
        assert_eq!(appended("").as_deref(), Some("@a "));
        assert_eq!(appended("fix").as_deref(), Some("fix @a "));
        assert_eq!(appended("fix ").as_deref(), Some("fix @a "));
    }

    #[test]
    fn offsets_are_utf16_units() {
        // `é` is one unit, `😀` two; a caret between a surrogate pair splits it.
        let text = "é😀 @x";
        let t = typed_token(text, 6, false, false).expect("token");
        assert_eq!((t.start, t.end, t.text.as_str()), (4, 6, "@x"));
        assert_eq!(typed_token("😀", 1, false, false), None);
        let s = pick(text, &t, "@xy").expect("pick");
        assert_eq!((s.text.as_str(), s.caret), ("é😀 @xy ", 8));
    }

    #[test]
    fn mentions_keep_the_picked_files_still_in_the_text() {
        let picked = vec!["@a.rs".into(), "@b.rs".into(), "@a.rs".into(), "/c".into()];
        assert_eq!(mentions("see @a.rs", picked), vec!["@a.rs".to_string()]);
    }
}
