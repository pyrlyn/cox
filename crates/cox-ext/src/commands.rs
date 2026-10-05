// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Slash commands from `.claude/commands/*.md` and `.cox/commands/*.md`
//! (T7.3): discovery, frontmatter, and body expansion. Shell and file
//! inclusion go through [`Includes`] so the binary can route `!` commands
//! through the `bash` tool and the permission engine — this crate never
//! spawns a process itself.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::frontmatter;

/// One `<name>.md` command file.
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    pub name: String,
    pub description: Option<String>,
    pub allowed_tools: Vec<String>,
    /// `model:` as written — a tier name, a Claude alias or a model id.
    pub model: Option<String>,
    pub argument_hint: Option<String>,
    pub path: PathBuf,
    pub body: String,
}

#[derive(Debug, Default, PartialEq)]
pub struct Discovered {
    /// Later directories replace earlier same-name commands (project over home).
    pub commands: Vec<Command>,
    pub notices: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Header {
    description: Option<String>,
    #[serde(rename = "allowed-tools")]
    allowed_tools: Option<serde_yaml::Value>,
    model: Option<String>,
    #[serde(rename = "argument-hint")]
    argument_hint: Option<String>,
}

/// `~/.cox/commands`, `~/.claude/commands`, `.cox/commands`, `.claude/commands`.
pub fn command_dirs(
    cox_home: Option<&Path>,
    claude_home: Option<&Path>,
    project: Option<&Path>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(h) = cox_home {
        dirs.push(h.join("commands"));
    }
    if let Some(h) = claude_home {
        dirs.push(h.join("commands"));
    }
    if let Some(p) = project {
        dirs.push(p.join(".cox").join("commands"));
        dirs.push(p.join(".claude").join("commands"));
    }
    dirs
}

/// Reads every `*.md` in each directory. A file without frontmatter is a
/// plain body (Claude allows that); a broken header is skipped with a notice.
pub fn discover(dirs: &[PathBuf]) -> Discovered {
    let mut found = Discovered::default();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "md") && p.is_file())
            .collect();
        paths.sort();
        for path in paths {
            match parse_command(&path) {
                Ok(cmd) => {
                    found.commands.retain(|c| c.name != cmd.name);
                    found.commands.push(cmd);
                }
                Err(reason) => found
                    .notices
                    .push(format!("command {} skipped: {reason}", path.display())),
            }
        }
    }
    found
}

fn parse_command(path: &Path) -> Result<Command, String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let name = path
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or("no file name")?;
    let (header, body) = match frontmatter::parse::<Header>(&text) {
        Ok(parsed) => parsed,
        Err(frontmatter::FrontmatterError::Missing) => (Header::default(), text.as_str()),
        Err(e) => return Err(e.to_string()),
    };
    Ok(Command {
        name,
        description: header.description,
        allowed_tools: frontmatter::names(header.allowed_tools.as_ref()),
        model: header.model,
        argument_hint: header.argument_hint,
        path: path.to_path_buf(),
        body: body.trim().to_string(),
    })
}

/// What expansion may pull in. The binary implements `shell` over the
/// `bash` tool (engine included) and `file` over the confined reader.
pub trait Includes {
    fn shell(&mut self, command: &str) -> Result<String, String>;
    fn file(&mut self, path: &str) -> Result<String, String>;
}

/// Expands `$ARGUMENTS`, `$1..$n`, `` !`cmd` `` and `@file` in the body.
/// Failed inclusions stay as written and are reported in `notices`.
pub fn expand(cmd: &Command, args: &str, inc: &mut dyn Includes) -> (String, Vec<String>) {
    let words: Vec<&str> = args.split_whitespace().collect();
    let mut notices = Vec::new();
    let mut out = String::with_capacity(cmd.body.len());
    let mut rest = cmd.body.as_str();
    while let Some(i) = rest.find(['$', '!', '@']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let (piece, used) = match tail.as_bytes()[0] {
            b'$' if tail.starts_with("$ARGUMENTS") => (args.to_string(), "$ARGUMENTS".len()),
            b'$' => {
                let digits: String = tail[1..].chars().take_while(char::is_ascii_digit).collect();
                match digits.parse::<usize>() {
                    Ok(n) if n >= 1 => (
                        words.get(n - 1).copied().unwrap_or("").to_string(),
                        1 + digits.len(),
                    ),
                    _ => ("$".to_string(), 1),
                }
            }
            b'!' if tail.starts_with("!`") => match tail[2..].find('`') {
                Some(end) => {
                    let command = &tail[2..2 + end];
                    match inc.shell(command) {
                        Ok(text) => (text.trim_end().to_string(), 3 + end),
                        Err(e) => {
                            notices.push(format!("command `{}`: !`{command}`: {e}", cmd.name));
                            (tail[..3 + end].to_string(), 3 + end)
                        }
                    }
                }
                None => ("!".to_string(), 1),
            },
            b'@' if word_start(&out) => {
                let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
                let path = &tail[1..end];
                match inc.file(path) {
                    Ok(text) if !path.is_empty() => (text.trim_end().to_string(), end),
                    _ => (tail[..end].to_string(), end),
                }
            }
            _ => (tail[..1].to_string(), 1),
        };
        out.push_str(&piece);
        rest = &rest[i + used..];
    }
    out.push_str(rest);
    (out, notices)
}

/// `@` counts as an include only at the start of a word (not `a@b.com`).
fn word_start(out: &str) -> bool {
    out.chars().next_back().is_none_or(char::is_whitespace)
}
