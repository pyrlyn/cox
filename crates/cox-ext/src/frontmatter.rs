// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! YAML frontmatter (`---` … `---`) shared by skills, commands and agent
//! definitions. One place to split and parse so every extension file fails
//! the same way: a malformed header is an error the caller turns into a
//! `Notice`, never a panic and never a silently empty definition.

use serde::de::DeserializeOwned;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum FrontmatterError {
    #[error("no frontmatter block (expected `---` on the first line)")]
    Missing,
    #[error("unterminated frontmatter (no closing `---`)")]
    Unterminated,
    #[error("frontmatter: {0}")]
    Yaml(String),
}

/// Splits `text` into its YAML header and the body after the closing `---`.
pub fn split(text: &str) -> Result<(&str, &str), FrontmatterError> {
    let rest = text
        .strip_prefix("---\r\n")
        .or_else(|| text.strip_prefix("---\n"))
        .ok_or(FrontmatterError::Missing)?;
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            return Ok((&rest[..offset], &rest[offset + line.len()..]));
        }
        offset += line.len();
    }
    Err(FrontmatterError::Unterminated)
}

/// Parses the header into `T` and returns it with the body.
pub fn parse<T: DeserializeOwned>(text: &str) -> Result<(T, &str), FrontmatterError> {
    let (yaml, body) = split(text)?;
    let value = serde_yaml::from_str(yaml).map_err(|e| FrontmatterError::Yaml(e.to_string()))?;
    Ok((value, body))
}

/// A field that Claude writes either as a YAML list or as one string of
/// space- or comma-separated names (`allowed-tools: Read Bash`).
pub fn names(value: Option<&serde_yaml::Value>) -> Vec<String> {
    match value {
        Some(serde_yaml::Value::Sequence(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::trim).map(String::from))
            .filter(|s| !s.is_empty())
            .collect(),
        Some(serde_yaml::Value::String(s)) => s
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_splits_header_from_body() {
        let (yaml, body) = split("---\nname: x\n---\nbody\n").unwrap();
        assert_eq!(yaml, "name: x\n");
        assert_eq!(body, "body\n");
        assert_eq!(split("no header"), Err(FrontmatterError::Missing));
        assert_eq!(split("---\nname: x\n"), Err(FrontmatterError::Unterminated));
    }

    #[test]
    fn frontmatter_names_accepts_list_and_string_forms() {
        let list: serde_yaml::Value = serde_yaml::from_str("- read\n- grep\n").unwrap();
        assert_eq!(names(Some(&list)), ["read", "grep"]);
        let text = serde_yaml::Value::String("Read, Bash grep".into());
        assert_eq!(names(Some(&text)), ["Read", "Bash", "grep"]);
        assert!(names(None).is_empty());
    }
}
