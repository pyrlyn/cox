// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The Settings Permissions page's model (T37.45.3, A120): the allow, ask
//! and deny rules in effect with the layer each list comes from, an edit
//! of one rule written to the user file only, and the `AllowForSession`
//! grants of the sessions open here. Separate from `settings.rs`, which
//! owns every leaf generically; this owns what a rule list means. It
//! decides nothing about a call: a rule is checked with `cox_permission`'s
//! own grammar before it is saved, the write is `settings::set` (the user
//! layer, refused once a layer above sets the list), and a revoke is a
//! `Submission` the core runs, so the engine stays the one place a call
//! is decided.

use std::path::Path;

use cox_core::permission::rules::Rule;
use cox_protocol::ids::SessionId;
use serde::Serialize;
use serde_json::Value;

use crate::settings::{self, Layer, Setting, SettingsError, SettingsView};

/// Which list a rule sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    Allow,
    Ask,
    Deny,
}

impl RuleKind {
    /// In the engine's order: a deny rule wins, then allow, then ask.
    pub const ALL: [Self; 3] = [Self::Deny, Self::Allow, Self::Ask];

    /// The dotted key of this kind's list.
    pub fn key(self) -> &'static str {
        match self {
            Self::Allow => "permissions.allow",
            Self::Ask => "permissions.ask",
            Self::Deny => "permissions.deny",
        }
    }
}

/// One rule in effect.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PermissionRule {
    pub kind: RuleKind,
    /// As written, the grammar's text.
    pub rule: String,
    /// A layer replaces a whole list, so every rule of a kind shares it.
    pub layer: Layer,
    /// Whether an edit to the user file takes effect (`Setting::editable`).
    pub editable: bool,
}

/// One `AllowForSession` grant of a session open here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionGrant {
    pub session: SessionId,
    /// The session's title, when it has one.
    pub title: Option<String>,
    pub tool: String,
    /// The subject prefix the grant covers.
    pub subject: String,
}

/// The rules the effective config holds, deny first, from `settings`'
/// list rows.
pub fn rules(settings: &[Setting]) -> Vec<PermissionRule> {
    RuleKind::ALL
        .into_iter()
        .filter_map(|kind| {
            settings
                .iter()
                .find(|s| s.key == kind.key())
                .map(|s| (kind, s))
        })
        .flat_map(|(kind, row)| {
            strings(&row.value)
                .into_iter()
                .map(move |rule| PermissionRule {
                    kind,
                    rule,
                    layer: row.layer,
                    editable: row.editable,
                })
        })
        .collect()
}

/// Adds (`old: None`), replaces or removes (`new: None`) one rule of
/// `kind` in the user file and returns the new view. `new` must parse
/// with the engine's grammar. The list written is the one in effect, so
/// a default rule (`Read(~/.ssh/**)`) is carried into the user file
/// rather than dropped by it; a list a higher layer sets is refused as
/// read-only by `settings::set`, so a project file never gains a rule here.
pub fn edit(
    user_file: &Path,
    cwd: &Path,
    kind: RuleKind,
    old: Option<&str>,
    new: Option<&str>,
) -> Result<SettingsView, SettingsError> {
    let new = new.map(str::trim);
    if let Some(raw) = new {
        let home = std::env::home_dir();
        Rule::parse(raw, home.as_deref(), cwd).map_err(|message| SettingsError::Rule {
            rule: raw.to_string(),
            message,
        })?;
    }
    let view = settings::view(user_file, cwd)?;
    let row = view
        .settings
        .iter()
        .find(|s| s.key == kind.key())
        .ok_or_else(|| SettingsError::Unknown(kind.key().to_string()))?;
    let mut list = strings(&row.value);
    match (old, new) {
        (Some(old), new) => {
            let at = list
                .iter()
                .position(|r| r == old)
                .ok_or_else(|| SettingsError::NoRule(old.to_string()))?;
            match new {
                Some(new) => list[at] = new.to_string(),
                None => {
                    list.remove(at);
                }
            }
        }
        (None, Some(new)) if !list.iter().any(|r| r == new) => list.push(new.to_string()),
        (None, _) => return Ok(view),
    }
    settings::set(user_file, cwd, kind.key(), &serde_json::to_string(&list)?)
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|all| {
            all.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    /// A user file with one allow rule and a git checkout whose project
    /// file sets its own ask list.
    fn scratch() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let user = dir.path().join("home/config.toml");
        let project = dir.path().join("project");
        fs::create_dir_all(user.parent().expect("parent")).expect("home");
        fs::create_dir_all(project.join(".git")).expect(".git");
        fs::create_dir_all(project.join(".cox")).expect(".cox");
        fs::write(
            &user,
            "# mine\n[permissions]\nallow = [\"Bash(cargo nextest:*)\"]\n",
        )
        .expect("user file");
        fs::write(
            project.join(".cox/config.toml"),
            "[permissions]\nask = [\"Edit(**/*.lock)\"]\n",
        )
        .expect("project file");
        (dir, user, project)
    }

    fn listed(view: &SettingsView) -> Vec<(RuleKind, &str, Layer, bool)> {
        view.rules
            .iter()
            .map(|r| (r.kind, r.rule.as_str(), r.layer, r.editable))
            .collect()
    }

    #[test]
    fn each_rule_is_listed_with_the_layer_its_list_comes_from() {
        let (_dir, user, project) = scratch();
        let view = settings::view(&user, &project).expect("view");
        assert_eq!(
            listed(&view),
            [
                (RuleKind::Deny, "Read(~/.ssh/**)", Layer::Default, true),
                (RuleKind::Deny, "Read(~/.aws/**)", Layer::Default, true),
                (RuleKind::Deny, "Bash(rm -rf /*)", Layer::Default, true),
                (RuleKind::Allow, "Bash(cargo nextest:*)", Layer::User, true),
                (RuleKind::Ask, "Edit(**/*.lock)", Layer::Project, false),
            ]
        );
    }

    #[test]
    fn an_invalid_rule_is_refused_with_the_grammars_message() {
        let (_dir, user, project) = scratch();
        let before = fs::read(&user).expect("read");
        let err = edit(
            &user,
            &project,
            RuleKind::Allow,
            None,
            Some("Bash(git push"),
        )
        .expect_err("unclosed");
        assert_eq!(
            err.to_string(),
            "`Bash(git push` is not a rule: missing closing ')'"
        );
        let empty = edit(&user, &project, RuleKind::Deny, None, Some(" ")).expect_err("empty");
        assert!(empty.to_string().ends_with("empty tool name"), "{empty}");
        assert_eq!(fs::read(&user).expect("read"), before);
    }

    #[test]
    fn a_valid_rule_lands_in_the_user_layer_only() {
        let (_dir, user, project) = scratch();
        let project_file = project.join(".cox/config.toml");
        let project_before = fs::read(&project_file).expect("read");
        let view = edit(
            &user,
            &project,
            RuleKind::Deny,
            None,
            Some("Bash(git push:*)"),
        )
        .expect("add");
        let deny: Vec<_> = listed(&view)
            .into_iter()
            .filter(|r| r.0 == RuleKind::Deny)
            .collect();
        // The defaults it replaced are carried into the user list.
        assert_eq!(
            deny,
            [
                (RuleKind::Deny, "Read(~/.ssh/**)", Layer::User, true),
                (RuleKind::Deny, "Read(~/.aws/**)", Layer::User, true),
                (RuleKind::Deny, "Bash(rm -rf /*)", Layer::User, true),
                (RuleKind::Deny, "Bash(git push:*)", Layer::User, true),
            ]
        );
        let written = fs::read_to_string(&user).expect("read");
        assert!(written.starts_with("# mine\n"), "{written}");
        assert!(written.contains("Bash(git push:*)"), "{written}");
        assert_eq!(fs::read(&project_file).expect("read"), project_before);
    }

    #[test]
    fn a_list_the_project_sets_is_read_only_here() {
        let (_dir, user, project) = scratch();
        let project_file = project.join(".cox/config.toml");
        let project_before = fs::read(&project_file).expect("read");
        let user_before = fs::read(&user).expect("read");
        let err = edit(
            &user,
            &project,
            RuleKind::Ask,
            None,
            Some("Bash(git push:*)"),
        )
        .expect_err("read-only");
        assert!(matches!(
            err,
            SettingsError::ReadOnly {
                layer: Layer::Project,
                ..
            }
        ));
        assert_eq!(fs::read(&project_file).expect("read"), project_before);
        assert_eq!(fs::read(&user).expect("read"), user_before);
    }

    #[test]
    fn a_rule_is_replaced_and_removed_in_place() {
        let (_dir, user, project) = scratch();
        let old = "Bash(cargo nextest:*)";
        let view = edit(
            &user,
            &project,
            RuleKind::Allow,
            Some(old),
            Some("Bash(cargo test:*)"),
        )
        .expect("replace");
        assert!(listed(&view).contains(&(
            RuleKind::Allow,
            "Bash(cargo test:*)",
            Layer::User,
            true
        )));
        let view = edit(
            &user,
            &project,
            RuleKind::Allow,
            Some("Bash(cargo test:*)"),
            None,
        )
        .expect("remove");
        assert!(!view.rules.iter().any(|r| r.kind == RuleKind::Allow));
        let missing = edit(&user, &project, RuleKind::Allow, Some(old), None).expect_err("gone");
        assert!(matches!(missing, SettingsError::NoRule(_)));
    }
}
