// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Harness entries on disk (T66.5, P66): the four refinable kinds — prompt
//! notes, memory facts, skills and subagents — applied with a backup first,
//! undone byte for byte, and fingerprinted by content. It lives here, not in
//! `cox-core`, because `cox-ext` already owns where each kind is read from
//! (`memory.rs`, `skills.rs`, `agents.rs`) and is the only crate allowed to
//! write those layouts. Proposing edits (T66.6) and the session wiring
//! (T66.7) are other cards: this module trusts nothing it is given, so an
//! id, an action or a path from a model is checked here before any write.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::memory;

/// Directory, under the scope root, that holds one backup per event.
pub const BACKUP_DIR: &str = "harness-backup";
const MANIFEST: &str = "manifest.json";

/// The refinable kinds. The base system prompt is deliberately not one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Memory,
    PromptNote,
    Skill,
    Subagent,
}

impl Kind {
    /// In the order of their names, which is the fingerprint's sort order.
    pub const ALL: [Kind; 4] = [Kind::Memory, Kind::PromptNote, Kind::Skill, Kind::Subagent];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Memory => "memory",
            Kind::PromptNote => "prompt-note",
            Kind::Skill => "skill",
            Kind::Subagent => "subagent",
        }
    }

    pub fn parse(word: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == word)
    }
}

/// What an edit does to its entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Create,
    Update,
    Delete,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Create => "create",
            Action::Update => "update",
            Action::Delete => "delete",
        }
    }
}

/// One requested change. `content` is the whole file for create and update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub action: Action,
    pub kind: Kind,
    pub id: String,
    pub content: Option<String>,
}

/// Where entries live. Local stays inside one session; global writes where
/// cox already reads each kind, so every later session sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// `<root>/harness/<kind>/<id>.md`, with `root` the session directory.
    Local { root: PathBuf },
    /// `home` is `~/.cox`; `memory_dir` is the project's memory directory.
    Global { home: PathBuf, memory_dir: PathBuf },
}

/// What one edit did, in the words and hashes `refine_events` stores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditRecord {
    pub action: Action,
    pub kind: Kind,
    pub id: String,
    pub before_sha: Option<String>,
    pub after_sha: Option<String>,
}

/// The result of one `apply`: the scope fingerprint either side of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub event_id: String,
    pub before: String,
    pub after: String,
    pub edits: Vec<EditRecord>,
}

/// One entry as it is on disk now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub kind: Kind,
    pub id: String,
    pub content: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("invalid harness id {0:?}: use [a-z0-9-], 1 to 64 characters")]
    InvalidId(String),
    #[error("no edits to apply")]
    Empty,
    #[error("two edits name {kind} {id:?}")]
    Duplicate { kind: &'static str, id: String },
    #[error("cannot {action} {kind} {id:?}: {why}")]
    Refused {
        action: &'static str,
        kind: &'static str,
        id: String,
        why: &'static str,
    },
    #[error("a backup for event {0:?} already exists")]
    EventExists(String),
    #[error("no backup for event {0:?}")]
    NoBackup(String),
    #[error("backup manifest of event {event:?} is unreadable: {why}")]
    Manifest { event: String, why: String },
    #[error("memory index: {0}")]
    Memory(String),
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
}

#[derive(Serialize, Deserialize)]
struct ManifestEntry {
    kind: Kind,
    id: String,
    existed: bool,
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> HarnessError + '_ {
    move |source| HarnessError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn valid_id(id: &str) -> Result<(), HarnessError> {
    if memory::is_valid_name(id) {
        Ok(())
    } else {
        Err(HarnessError::InvalidId(id.to_string()))
    }
}

/// Lowercase hex sha256 of a file's bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(digest: &[u8]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

impl Scope {
    fn name(&self) -> &'static str {
        match self {
            Scope::Local { .. } => "local",
            Scope::Global { .. } => "global",
        }
    }

    /// Backups sit beside the entries' scope, never inside a kind directory.
    fn root(&self) -> &Path {
        match self {
            Scope::Local { root } => root,
            Scope::Global { home, .. } => home,
        }
    }

    fn kind_dir(&self, kind: Kind) -> PathBuf {
        match (self, kind) {
            (Scope::Local { root }, k) => root.join("harness").join(k.as_str()),
            (Scope::Global { memory_dir, .. }, Kind::Memory) => memory_dir.clone(),
            (Scope::Global { home, .. }, Kind::PromptNote) => home.join("notes"),
            (Scope::Global { home, .. }, Kind::Skill) => home.join("skills"),
            (Scope::Global { home, .. }, Kind::Subagent) => home.join("agents"),
        }
    }

    /// Callers validate `id` first, so the result cannot leave `kind_dir`.
    fn entry_path(&self, kind: Kind, id: &str) -> PathBuf {
        let dir = self.kind_dir(kind);
        match (self, kind) {
            (Scope::Global { .. }, Kind::Skill) => dir.join(id).join("SKILL.md"),
            _ => dir.join(format!("{id}.md")),
        }
    }

    fn ids(&self, kind: Kind) -> Vec<String> {
        let Ok(read) = fs::read_dir(self.kind_dir(kind)) else {
            return Vec::new();
        };
        let global_skill = matches!((self, kind), (Scope::Global { .. }, Kind::Skill));
        let mut ids: Vec<String> = read
            .flatten()
            // A symlink could point a read or a write out of the scope.
            .filter(|e| e.file_type().is_ok_and(|t| !t.is_symlink()))
            .filter_map(|e| {
                let path = e.path();
                if global_skill {
                    return path
                        .join("SKILL.md")
                        .is_file()
                        .then(|| e.file_name().to_string_lossy().into_owned());
                }
                let stem = path.file_stem()?.to_string_lossy().into_owned();
                (path.extension()? == "md" && path.is_file()).then_some(stem)
            })
            .filter(|id| memory::is_valid_name(id))
            .collect();
        ids.sort();
        ids
    }

    fn touches_memory_index(&self, kinds: impl IntoIterator<Item = Kind>) -> bool {
        matches!(self, Scope::Global { .. }) && kinds.into_iter().any(|k| k == Kind::Memory)
    }

    fn rebuild_memory_index(&self) -> Result<(), HarnessError> {
        match self {
            Scope::Global { memory_dir, .. } => memory::rebuild_index(memory_dir)
                .map(|_| ())
                .map_err(HarnessError::Memory),
            Scope::Local { .. } => Ok(()),
        }
    }

    fn backup_dir(&self, event_id: &str) -> PathBuf {
        self.root().join(BACKUP_DIR).join(event_id)
    }

    /// Every entry now on disk, in `kind`, `id` order.
    pub fn load(&self) -> Result<Vec<Loaded>, HarnessError> {
        let mut out = Vec::new();
        for kind in Kind::ALL {
            for id in self.ids(kind) {
                let path = self.entry_path(kind, &id);
                let content = fs::read(&path).map_err(io_err(&path))?;
                out.push(Loaded { kind, id, content });
            }
        }
        Ok(out)
    }

    /// sha256 over the entries sorted by `scope\0kind\0id`, with their
    /// content. A content length prefix keeps one entry's tail from reading
    /// as the next entry's head.
    pub fn fingerprint(&self) -> Result<String, HarnessError> {
        let mut hasher = Sha256::new();
        for entry in self.load()? {
            hasher.update(self.name().as_bytes());
            hasher.update([0]);
            hasher.update(entry.kind.as_str().as_bytes());
            hasher.update([0]);
            hasher.update(entry.id.as_bytes());
            hasher.update([0]);
            hasher.update((entry.content.len() as u64).to_le_bytes());
            hasher.update(&entry.content);
        }
        Ok(hex(&hasher.finalize()))
    }

    /// Checks every edit before the first byte is written, so a refused
    /// proposal leaves the scope untouched.
    fn preflight(&self, edits: &[Edit]) -> Result<Vec<Option<Vec<u8>>>, HarnessError> {
        if edits.is_empty() {
            return Err(HarnessError::Empty);
        }
        let mut seen = std::collections::HashSet::new();
        let mut previous = Vec::with_capacity(edits.len());
        for edit in edits {
            valid_id(&edit.id)?;
            if !seen.insert((edit.kind, edit.id.as_str())) {
                return Err(HarnessError::Duplicate {
                    kind: edit.kind.as_str(),
                    id: edit.id.clone(),
                });
            }
            let refuse = |why| HarnessError::Refused {
                action: edit.action.as_str(),
                kind: edit.kind.as_str(),
                id: edit.id.clone(),
                why,
            };
            let path = self.entry_path(edit.kind, &edit.id);
            let old = match fs::symlink_metadata(&path) {
                Ok(m) if m.file_type().is_symlink() => return Err(refuse("path is a symlink")),
                Ok(_) => Some(fs::read(&path).map_err(io_err(&path))?),
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(e) => return Err(io_err(&path)(e)),
            };
            match (edit.action, old.is_some(), edit.content.is_some()) {
                (Action::Create, true, _) => return Err(refuse("it already exists")),
                (Action::Update | Action::Delete, false, _) => {
                    return Err(refuse("it does not exist"));
                }
                (Action::Create | Action::Update, _, false) => {
                    return Err(refuse("content is required"));
                }
                (Action::Delete, _, true) => return Err(refuse("a delete carries no content")),
                _ => {}
            }
            previous.push(old);
        }
        Ok(previous)
    }

    /// Applies `edits` after copying every file they touch to
    /// `<root>/harness-backup/<event_id>/`. A write that fails halfway is
    /// rolled back before the error is returned.
    pub fn apply(&self, event_id: &str, edits: &[Edit]) -> Result<Applied, HarnessError> {
        valid_id(event_id)?;
        let previous = self.preflight(edits)?;
        let backup = self.backup_dir(event_id);
        if backup.exists() {
            return Err(HarnessError::EventExists(event_id.to_string()));
        }
        let before = self.fingerprint()?;
        let manifest: Vec<ManifestEntry> = edits
            .iter()
            .zip(&previous)
            .map(|(e, old)| ManifestEntry {
                kind: e.kind,
                id: e.id.clone(),
                existed: old.is_some(),
            })
            .collect();
        for (edit, old) in edits.iter().zip(&previous) {
            if let Some(bytes) = old {
                let copy = backup_file(&backup, edit.kind, &edit.id);
                write_with_parents(&copy, bytes)?;
            }
        }
        let manifest_path = backup.join(MANIFEST);
        let json = serde_json::to_vec(&manifest).map_err(|e| HarnessError::Manifest {
            event: event_id.to_string(),
            why: e.to_string(),
        })?;
        write_with_parents(&manifest_path, &json)?;

        if let Err(e) = self.write_all(edits) {
            // The original failure is the one worth reporting.
            let _ = self.rollback(event_id);
            return Err(e);
        }
        let records = edits
            .iter()
            .zip(&previous)
            .map(|(e, old)| EditRecord {
                action: e.action,
                kind: e.kind,
                id: e.id.clone(),
                before_sha: old.as_deref().map(sha256_hex),
                after_sha: e.content.as_deref().map(|c| sha256_hex(c.as_bytes())),
            })
            .collect();
        Ok(Applied {
            event_id: event_id.to_string(),
            before,
            after: self.fingerprint()?,
            edits: records,
        })
    }

    fn write_all(&self, edits: &[Edit]) -> Result<(), HarnessError> {
        for edit in edits {
            let path = self.entry_path(edit.kind, &edit.id);
            match (&edit.content, edit.action) {
                (Some(content), Action::Create | Action::Update) => {
                    write_with_parents(&path, content.as_bytes())?;
                }
                _ => remove_entry(&path, self.is_skill_dir(edit.kind))?,
            }
        }
        if self.touches_memory_index(edits.iter().map(|e| e.kind)) {
            self.rebuild_memory_index()?;
        }
        Ok(())
    }

    fn is_skill_dir(&self, kind: Kind) -> bool {
        matches!((self, kind), (Scope::Global { .. }, Kind::Skill))
    }

    /// Puts back every file the event's backup holds and deletes entries the
    /// event created. The backup is consumed, so a second call cannot
    /// overwrite later work with stale bytes.
    pub fn rollback(&self, event_id: &str) -> Result<(), HarnessError> {
        valid_id(event_id)?;
        let backup = self.backup_dir(event_id);
        let manifest_path = backup.join(MANIFEST);
        let raw = match fs::read(&manifest_path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(HarnessError::NoBackup(event_id.to_string()));
            }
            Err(e) => return Err(io_err(&manifest_path)(e)),
        };
        let manifest: Vec<ManifestEntry> =
            serde_json::from_slice(&raw).map_err(|e| HarnessError::Manifest {
                event: event_id.to_string(),
                why: e.to_string(),
            })?;
        for entry in &manifest {
            // The manifest is a file on disk like any other: re-check the id.
            valid_id(&entry.id)?;
            let path = self.entry_path(entry.kind, &entry.id);
            if entry.existed {
                let copy = backup_file(&backup, entry.kind, &entry.id);
                let bytes = fs::read(&copy).map_err(io_err(&copy))?;
                write_with_parents(&path, &bytes)?;
            } else {
                remove_entry(&path, self.is_skill_dir(entry.kind))?;
            }
        }
        if self.touches_memory_index(manifest.iter().map(|e| e.kind)) {
            self.rebuild_memory_index()?;
        }
        fs::remove_dir_all(&backup).map_err(io_err(&backup))
    }
}

fn backup_file(backup: &Path, kind: Kind, id: &str) -> PathBuf {
    backup.join("files").join(kind.as_str()).join(id)
}

fn write_with_parents(path: &Path, bytes: &[u8]) -> Result<(), HarnessError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_err(parent))?;
    }
    fs::write(path, bytes).map_err(io_err(path))
}

/// Removes an entry file; a skill's now-empty directory goes with it, a
/// directory that holds anything else stays.
fn remove_entry(path: &Path, skill_dir: bool) -> Result<(), HarnessError> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_err(path)(e)),
    }
    if skill_dir && let Some(parent) = path.parent() {
        let _ = fs::remove_dir(parent);
    }
    Ok(())
}
