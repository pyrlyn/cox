// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The inspector's Info tab (DT§5.1, T37.29.5): what a session is and where
//! it lives — its id, cwd, linked worktree, the config layers it runs with
//! and its rollout file. Built on request from what already answers each
//! part (`cox_tools::git::linked`, the Settings view's `cox-config`
//! provenance, `cox_store::Store::rollout_path`); separate so the tab's
//! shape is tested apart from the live session that gathers it. The rows a
//! client lists are built here as [`Fact`]s (T58.4.20) so each client shows
//! the same words instead of deciding them again.

use std::path::{Path, PathBuf};

use cox_protocol::ids::SessionId;
use cox_tools::git::Linked;
use serde::{Deserialize, Serialize};

use crate::settings::{Layer, SettingsView};

/// What the Info tab lists.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Info {
    pub session: SessionId,
    pub cwd: PathBuf,
    /// `None` when the session runs outside a linked worktree.
    pub worktree: Option<Linked>,
    /// Each layer that set at least one key, in load order (`default`
    /// first, `flag` last).
    pub config: Vec<ConfigSource>,
    /// The session's JSONL rollout, where its events are appended.
    pub rollout: PathBuf,
    /// Id, folder, worktree and its branch, rollout — as the tab lists them.
    pub facts: Vec<Fact>,
    /// A row per layer with its key count, its file under it as a detail row.
    pub config_facts: Vec<Fact>,
}

/// One row of an inspector tab's fact list, in the order it is shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    pub label: String,
    /// `None` for a row that is only a label (a layer's file).
    pub value: Option<String>,
    /// Indented under the row above it.
    pub detail: bool,
}

impl Fact {
    pub(crate) fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: Some(value.into()),
            detail: false,
        }
    }

    fn detail(mut self) -> Self {
        self.detail = true;
        self
    }
}

/// A worktree's branch, `detached` without one.
pub(crate) fn branch_fact(tree: &Linked) -> Fact {
    Fact::new("Branch", tree.branch.as_deref().unwrap_or("detached"))
}

/// One config layer the session's config came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigSource {
    pub layer: Layer,
    /// The file it was read from; `None` for a layer that is not a file.
    pub file: Option<PathBuf>,
    /// How many effective leaves it set.
    pub keys: u32,
}

/// The order `cox-config` layers them in, later over earlier.
const ORDER: [Layer; 6] = [
    Layer::Default,
    Layer::User,
    Layer::Project,
    Layer::ClaudeSettings,
    Layer::Env,
    Layer::Flag,
];

/// `home` is shown as `~` in every path.
pub fn build(
    session: SessionId,
    cwd: &Path,
    worktree: Option<Linked>,
    settings: &SettingsView,
    rollout: PathBuf,
    home: &Path,
) -> Info {
    let config: Vec<ConfigSource> = ORDER
        .into_iter()
        .filter_map(|layer| {
            let n = settings
                .settings
                .iter()
                .filter(|s| s.layer == layer)
                .count();
            let file = match layer {
                Layer::User => Some(settings.user_file.clone()),
                Layer::Project => settings.project_file.clone(),
                _ => None,
            };
            (n > 0).then(|| ConfigSource {
                layer,
                file,
                keys: u32::try_from(n).unwrap_or(u32::MAX),
            })
        })
        .collect();
    let path = |full: &Path| tilde(full, home);
    let mut facts = vec![
        Fact::new("Session", session.to_string()),
        Fact::new("Folder", path(cwd)),
    ];
    if let Some(tree) = &worktree {
        facts.push(Fact::new("Worktree", path(&tree.path)));
        facts.push(branch_fact(tree).detail());
    }
    facts.push(Fact::new("Rollout", path(&rollout)));
    let config_facts = config.iter().flat_map(|s| layer_facts(s, home)).collect();
    Info {
        session,
        cwd: cwd.to_path_buf(),
        worktree,
        config,
        rollout,
        facts,
        config_facts,
    }
}

/// A layer's `N keys` row, then its file as a detail row.
fn layer_facts(source: &ConfigSource, home: &Path) -> Vec<Fact> {
    let unit = if source.keys == 1 { "key" } else { "keys" };
    let count = Fact::new(source.layer.to_string(), format!("{} {unit}", source.keys));
    let file = source.file.as_deref().map(|file| Fact {
        label: tilde(file, home),
        value: None,
        detail: true,
    });
    std::iter::once(count).chain(file).collect()
}

/// `full` with a leading `home` written as `~`.
fn tilde(full: &Path, home: &Path) -> String {
    match full.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => Path::new("~").join(rest).display().to_string(),
        Err(_) => full.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn info_counts_each_layer_that_set_a_key_with_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let (home, repo) = (dir.path().join("home"), dir.path().join("repo"));
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(repo.join(".cox")).unwrap();
        fs::create_dir_all(&home).unwrap();
        let user = home.join("config.toml");
        fs::write(&user, "[budget]\nwarn_at = 0.5\n").unwrap();
        let project = repo.join(".cox").join("config.toml");
        fs::write(&project, "[tui]\nvim = true\n").unwrap();
        let settings = crate::settings::view(&user, &repo).unwrap();
        let id = SessionId::new();
        let rollout = home.join("sessions").join(format!("{id}.jsonl"));

        let info = build(
            id,
            &repo,
            None,
            &settings,
            rollout.clone(),
            Path::new("/nowhere"),
        );

        let layers: Vec<_> = info
            .config
            .iter()
            // A `COX_*` variable in the developer's shell adds an env layer.
            .filter(|c| c.layer != Layer::Env)
            .map(|c| (c.layer, c.file.clone()))
            .collect();
        let project = fs::canonicalize(&repo).unwrap().join(".cox/config.toml");
        assert_eq!(
            layers,
            [
                (Layer::Default, None),
                (Layer::User, Some(user)),
                (Layer::Project, Some(project)),
            ]
        );
        assert_eq!(info.config[1].keys, 1);
        assert_eq!(info.config[2].keys, 1);
        assert!(info.config[0].keys > 1);
        assert_eq!((info.session, info.cwd, info.rollout), (id, repo, rollout));
    }

    fn rows(facts: &[Fact]) -> Vec<(&str, Option<&str>, bool)> {
        facts
            .iter()
            .map(|f| (f.label.as_str(), f.value.as_deref(), f.detail))
            .collect()
    }

    #[test]
    fn the_home_directory_reads_as_tilde() {
        let (home, repo) = (Path::new("/Users/me"), Path::new("/Users/me/code/cox"));
        let settings = SettingsView {
            settings: Vec::new(),
            user_file: home.join(".cox/config.toml"),
            project_file: None,
            mcp: Vec::new(),
            dropped: Vec::new(),
            rules: Vec::new(),
            grants: Vec::new(),
            providers: Vec::new(),
        };
        let tree = Linked {
            path: PathBuf::from("/Users/me/wt"),
            branch: None,
            base: None,
            commit: None,
            bytes: 0,
        };
        let id = SessionId::new();
        let rollout = PathBuf::from("/var/cox/r.jsonl");

        let info = build(id, repo, Some(tree), &settings, rollout, home);

        let id = id.to_string();
        assert_eq!(
            rows(&info.facts),
            [
                ("Session", Some(id.as_str()), false),
                ("Folder", Some("~/code/cox"), false),
                ("Worktree", Some("~/wt"), false),
                ("Branch", Some("detached"), true),
                ("Rollout", Some("/var/cox/r.jsonl"), false),
            ]
        );
        assert_eq!(tilde(home, home), "~");
        assert_eq!(tilde(Path::new("/Users/meadow"), home), "/Users/meadow");
    }

    #[test]
    fn a_layer_lists_its_key_count_and_its_file_under_it() {
        let config = [
            ConfigSource {
                layer: Layer::User,
                file: Some(PathBuf::from("/h/.cox/config.toml")),
                keys: 1,
            },
            ConfigSource {
                layer: Layer::Env,
                file: None,
                keys: 3,
            },
        ];
        let mut facts = Vec::new();
        for source in &config {
            facts.extend(layer_facts(source, Path::new("/h")));
        }
        assert_eq!(
            rows(&facts),
            [
                ("user", Some("1 key"), false),
                ("~/.cox/config.toml", None, true),
                ("env", Some("3 keys"), false),
            ]
        );
    }
}
