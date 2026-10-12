// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! YAML frontmatter (`---` … `---`) shared by skills, commands and agent
//! definitions. One place to split and parse so every extension file fails
//! the same way: a malformed header is an error the caller turns into a
//! `Notice`, never a panic and never a silently empty definition.

use serde::de::DeserializeOwned;

/// A header field whose shape varies (a list or a string, free-form
/// metadata). serde-saphyr has no value tree of its own and documents
/// `serde_json::Value` as its typeless target.
pub type Value = serde_json::Value;

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
    // Headers keep the YAML 1.2 reading they had before T7.9: `yes`/`on`
    // stay strings (so `disabled: yes` is an error, not true), and `.inf` in
    // a typeless field stays text instead of rejecting the whole file.
    let options = serde_saphyr::options! {
        strict_booleans: true,
        reject_non_finite_typeless_float: false,
    };
    // Without the source snippet: the message becomes a one-line `Notice`.
    let value = serde_saphyr::from_str_with_options(yaml, options)
        .map_err(|e| FrontmatterError::Yaml(e.without_snippet().to_string()))?;
    Ok((value, body))
}

/// A free-form field as text: strings verbatim, anything else as YAML.
pub fn text(value: Value) -> String {
    match value {
        Value::String(s) => s,
        other => serde_saphyr::to_string(&other)
            .unwrap_or_default()
            .trim_end()
            .to_string(),
    }
}

/// A field that Claude writes either as a YAML list or as one string of
/// space- or comma-separated names (`allowed-tools: Read Bash`).
pub fn names(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::trim).map(String::from))
            .filter(|s| !s.is_empty())
            .collect(),
        Some(Value::String(s)) => s
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
        let list: Value = serde_saphyr::from_str("- read\n- grep\n").unwrap();
        assert_eq!(names(Some(&list)), ["read", "grep"]);
        let text = Value::String("Read, Bash grep".into());
        assert_eq!(names(Some(&text)), ["Read", "Bash", "grep"]);
        assert!(names(None).is_empty());
    }

    #[derive(Debug, serde::Deserialize)]
    struct Probe {
        tools: Option<Value>,
        disabled: Option<bool>,
        metadata: Option<std::collections::BTreeMap<String, Value>>,
    }

    fn probe(yaml: &str) -> Result<Probe, FrontmatterError> {
        parse::<Probe>(&format!("---\n{yaml}---\n")).map(|(p, _)| p)
    }

    #[test]
    fn frontmatter_reads_yaml_1_2_booleans_as_strings() {
        let p = probe("tools: yes\n").unwrap();
        assert_eq!(names(p.tools.as_ref()), ["yes"]);
        assert!(matches!(
            probe("disabled: yes\n"),
            Err(FrontmatterError::Yaml(_))
        ));
        assert!(matches!(
            probe("disabled: off\n"),
            Err(FrontmatterError::Yaml(_))
        ));
        assert_eq!(probe("disabled: True\n").unwrap().disabled, Some(true));
    }

    #[test]
    fn frontmatter_names_ignores_non_string_shapes() {
        let p = probe("tools: [read, 5, ' x ', '']\n").unwrap();
        assert_eq!(names(p.tools.as_ref()), ["read", "x"]);
        for yaml in ["tools: 5\n", "tools: ~\n", "tools: {1: b}\n"] {
            assert!(
                names(probe(yaml).unwrap().tools.as_ref()).is_empty(),
                "{yaml}"
            );
        }
    }

    #[test]
    fn frontmatter_text_renders_non_strings_as_yaml() {
        let yaml =
            "metadata:\n  v: 1.0\n  beta: yes\n  inf: .inf\n  list: [a, b]\n  map: {z: 1, a: 2}\n";
        let text: Vec<(String, String)> = probe(yaml)
            .unwrap()
            .metadata
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| (k, text(v)))
            .collect();
        let want = [
            ("beta", "yes"),
            ("inf", ".inf"),
            ("list", "- a\n- b"),
            ("map", "z: 1\na: 2"),
            ("v", "1.0"),
        ];
        let want: Vec<(String, String)> = want
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(text, want);
    }

    #[test]
    fn frontmatter_error_is_one_line() {
        let Err(FrontmatterError::Yaml(msg)) = probe("disabled: yes\n") else {
            panic!("expected a YAML error");
        };
        assert!(!msg.contains('\n'), "{msg}");
        assert!(msg.contains("line 1"), "{msg}");
    }

    /// The one reading that changed with T7.9: serde_yaml refused a quoted
    /// `'true'` for a bool field, serde-saphyr accepts it.
    #[test]
    fn frontmatter_quoted_true_is_a_bool() {
        assert_eq!(probe("disabled: 'true'\n").unwrap().disabled, Some(true));
    }
}
