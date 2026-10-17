// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Which targets are running (T68.4): the listing command of each launcher
//! (`docker ps`, `limactl list`, `colima list`) and the reading of what it
//! prints. Separate from `command` because a listing is local argv with no
//! remote part, and from the caller because the parsing is pure and pinned to
//! sample outputs here. Output is another program's text, so every name is
//! checked like a typed one and anything unreadable lists nothing.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::{Kind, Target, check_name, kept_env};

/// The command that lists `kind`'s running targets, not yet started; `None`
/// for ssh, which has nothing to list. The environment is scrubbed as for a
/// connection.
pub fn listing(kind: Kind, launcher: &Path) -> Option<Command> {
    let args: &[&str] = match kind {
        Kind::Ssh => return None,
        Kind::Docker => &["ps", "--format", "{{json .}}"],
        Kind::Lima | Kind::Colima => &["list", "--json"],
    };
    let mut command = Command::new(launcher);
    command.args(args).env_clear().envs(
        kept_env(kind)
            .iter()
            .filter_map(|k| Some((*k, std::env::var_os(k)?))),
    );
    Some(command)
}

/// The running targets in `output`, which is one JSON object per line (what
/// all three tools print) or one JSON array. A line that is not JSON, a
/// stopped VM and a name a launcher could misread are skipped.
pub fn parse_listing(kind: Kind, output: &str) -> Vec<Target> {
    let objects = output
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .flat_map(|v| match v {
            Value::Array(items) => items,
            one => vec![one],
        });
    let mut found: Vec<Target> = objects.filter_map(|o| target(kind, &o)).collect();
    found.dedup();
    found
}

fn target(kind: Kind, object: &Value) -> Option<Target> {
    let field = |key: &str| object.get(key)?.as_str();
    let name = match kind {
        // why: `docker ps` already lists running containers only; a
        // container with several names prints them comma-joined.
        Kind::Docker => field("Names")?.split(',').next()?,
        Kind::Lima | Kind::Colima => {
            if !field("status")?.eq_ignore_ascii_case("running") {
                return None;
            }
            field("name")?
        }
        Kind::Ssh => return None,
    };
    check_name(name).ok()?;
    let name = name.to_owned();
    Some(match kind {
        Kind::Docker => Target::Docker {
            container: name,
            context: None,
        },
        Kind::Lima => Target::Lima { instance: name },
        _ => Target::Colima { profile: name },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(kind: Kind, output: &str) -> Vec<String> {
        parse_listing(kind, output)
            .iter()
            .map(Target::to_string)
            .collect()
    }

    #[test]
    fn docker_ps_lines_list_each_container() {
        let out = concat!(
            r#"{"ID":"a1","Names":"web","State":"running","Image":"nginx"}"#,
            "\n",
            r#"{"ID":"b2","Names":"db,linked/db","State":"running"}"#,
            "\n"
        );
        assert_eq!(names(Kind::Docker, out), ["docker:web", "docker:db"]);
    }

    #[test]
    fn only_running_vms_are_listed() {
        let lima = concat!(
            r#"{"name":"default","status":"Running","arch":"aarch64"}"#,
            "\n",
            r#"{"name":"old","status":"Stopped"}"#,
            "\n"
        );
        assert_eq!(names(Kind::Lima, lima), ["lima:default"]);
        let colima = r#"[{"name":"dev","status":"Running"},{"name":"x","status":"Stopped"}]"#;
        assert_eq!(names(Kind::Colima, colima), ["colima:dev"]);
    }

    #[test]
    fn garbage_and_option_like_names_list_nothing() {
        assert!(names(Kind::Docker, "Cannot connect to the Docker daemon").is_empty());
        assert!(names(Kind::Docker, r#"{"Names":"-oProxyCommand=x"}"#).is_empty());
        assert!(names(Kind::Lima, r#"{"name":"a b","status":"Running"}"#).is_empty());
        assert!(names(Kind::Ssh, r#"{"name":"h","status":"Running"}"#).is_empty());
    }

    #[test]
    fn listing_commands_are_fixed_and_ssh_has_none() {
        let launcher = Path::new("/opt/docker");
        let args = |kind| {
            listing(kind, launcher)
                .expect("a listing")
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(args(Kind::Docker), ["ps", "--format", "{{json .}}"]);
        assert_eq!(args(Kind::Lima), ["list", "--json"]);
        assert_eq!(args(Kind::Colima), ["list", "--json"]);
        assert!(listing(Kind::Ssh, launcher).is_none());
    }
}
