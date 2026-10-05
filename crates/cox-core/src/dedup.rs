// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Re-read dedup (D6b, plan.md §1.3 step vi): an identical read-only call
//! within `context.dedup_window_turns` rounds, with no write to its subject
//! since, shows a pointer to the earlier archive row instead of the payload.
//! Separate from `truncate` because it is stateful per session and keyed by
//! the call, not by the size of its output.

use std::collections::{BTreeMap, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};

use cox_protocol::ArchiveId;
use cox_protocol::types::Risk;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    tool: String,
    input: String,
}

#[derive(Debug, Clone)]
struct Entry {
    id: ArchiveId,
    round: u32,
    subject: String,
    digest: u64,
}

/// Per-session table of recent read-only results.
#[derive(Debug)]
pub(crate) struct Dedup {
    window: u32,
    entries: HashMap<Key, Entry>,
}

impl Dedup {
    /// `window == 0` disables dedup entirely (T8.5's toggle).
    pub fn new(window: u32) -> Self {
        Self {
            window,
            entries: HashMap::new(),
        }
    }

    /// Records `output` for `(tool, input)` at `round` and returns the
    /// pointer text when the same call produced the same bytes within the
    /// window. The digest guard means an external change to the file still
    /// shows up even though cox never saw a write to it.
    pub fn observe(
        &mut self,
        tool: &str,
        input: &Value,
        subject: &str,
        id: ArchiveId,
        round: u32,
        output: &[u8],
    ) -> Option<String> {
        if self.window == 0 {
            return None;
        }
        let key = Key {
            tool: tool.to_owned(),
            input: canonical(input),
        };
        let digest = digest(output);
        let hit = self
            .entries
            .get(&key)
            .filter(|e| e.digest == digest && round.saturating_sub(e.round) < self.window)
            .map(|e| pointer(e.id, e.round));
        if hit.is_none() {
            self.entries.insert(
                key,
                Entry {
                    id,
                    round,
                    subject: subject.to_owned(),
                    digest,
                },
            );
        }
        hit
    }

    /// A `Write` drops entries whose subject overlaps `subject` as a path
    /// prefix; `Exec` and `Destructive` drop everything, since a command can
    /// touch any path.
    pub fn invalidate(&mut self, risk: Risk, subject: &str) {
        match risk {
            Risk::ReadOnly => {}
            Risk::Write => self.entries.retain(|_, e| !overlaps(&e.subject, subject)),
            Risk::Exec | Risk::Destructive => self.entries.clear(),
        }
    }
}

/// The text the model sees instead of the payload.
pub(crate) fn pointer(id: ArchiveId, round: u32) -> String {
    format!("unchanged since turn {round}, see #{id} (expand to re-show)")
}

fn overlaps(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && (a.starts_with(b) || b.starts_with(a))
}

fn digest(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// JSON with object keys sorted, so `{"a":1,"b":2}` and `{"b":2,"a":1}`
/// are the same read.
fn canonical(v: &Value) -> String {
    fn sort(v: &Value) -> Value {
        match v {
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), sort(v)))
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            Value::Array(items) => Value::Array(items.iter().map(sort).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_string(&sort(v)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn read(d: &mut Dedup, path: &str, round: u32, out: &[u8]) -> Option<String> {
        d.observe(
            "read",
            &json!({"path": path}),
            path,
            ArchiveId::new(),
            round,
            out,
        )
    }

    #[test]
    fn dedup_second_identical_read_is_a_pointer_to_the_first_archive() {
        let mut d = Dedup::new(8);
        let id = ArchiveId::new();
        assert!(
            d.observe("read", &json!({"path": "a"}), "a", id, 1, b"x")
                .is_none()
        );
        let hit = read(&mut d, "a", 3, b"x").expect("pointer");
        assert_eq!(
            hit,
            format!("unchanged since turn 1, see #{id} (expand to re-show)")
        );
        assert!(hit.len() < 120);
    }

    #[test]
    fn dedup_changed_output_and_expired_window_show_the_payload() {
        let mut d = Dedup::new(2);
        assert!(read(&mut d, "a", 1, b"x").is_none());
        assert!(read(&mut d, "a", 2, b"y").is_none(), "different bytes");
        assert!(read(&mut d, "a", 2, b"y").is_some());
        assert!(read(&mut d, "a", 4, b"y").is_none(), "outside the window");
        assert!(
            Dedup::new(0)
                .observe("read", &json!({}), "", ArchiveId::new(), 1, b"")
                .is_none()
        );
        let _ = read(&mut Dedup::new(0), "a", 1, b"x");
    }

    #[test]
    fn dedup_write_invalidates_by_path_prefix_and_exec_clears_all() {
        let mut d = Dedup::new(8);
        assert!(read(&mut d, "/w/src/a.rs", 1, b"x").is_none());
        assert!(read(&mut d, "/w/docs/b.md", 1, b"x").is_none());
        d.invalidate(Risk::Write, "/w/src");
        assert!(
            read(&mut d, "/w/src/a.rs", 1, b"x").is_none(),
            "invalidated"
        );
        assert!(read(&mut d, "/w/docs/b.md", 1, b"x").is_some(), "untouched");
        d.invalidate(Risk::Exec, "cargo build");
        assert!(
            read(&mut d, "/w/docs/b.md", 1, b"x").is_none(),
            "exec clears all"
        );
    }

    #[test]
    fn dedup_key_ignores_object_key_order() {
        let mut d = Dedup::new(8);
        let id = ArchiveId::new();
        assert!(
            d.observe("grep", &json!({"q": "x", "path": "p"}), "p", id, 1, b"o")
                .is_none()
        );
        assert!(
            d.observe("grep", &json!({"path": "p", "q": "x"}), "p", id, 1, b"o")
                .is_some()
        );
    }
}
