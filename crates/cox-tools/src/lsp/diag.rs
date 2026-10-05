// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The diagnostic subset of the LSP wire (`Position`, `Range`, `Diagnostic`,
//! `PublishDiagnosticsParams`), `file://` URI conversion and the one text
//! format the `diagnostics` tool returns. Separate from the transport
//! (`client`) and the server lifecycle (`server`) because it is pure data:
//! parsing and printing are tested without a pipe or a process.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

/// A zero-based line and UTF-16 column, as the server sends it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Deserialize)]
#[serde(default)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

/// One diagnostic. Fields cox does not print (`codeDescription`, `tags`,
/// `relatedInformation`, `data`) are ignored; missing optional ones default.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct Diagnostic {
    pub range: Range,
    /// 1 error, 2 warning, 3 information, 4 hint.
    pub severity: Option<u8>,
    /// A number or a string on the wire.
    pub code: Option<Value>,
    pub source: Option<String>,
    pub message: String,
}

/// `textDocument/publishDiagnostics` params, and the `items` of a pull
/// report share `Diagnostic`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct PublishDiagnosticsParams {
    pub uri: String,
    pub version: Option<i32>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Ordered most severe first, which is the order the formatter prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
            Self::Hint => "hint",
        }
    }
}

impl Diagnostic {
    /// A missing or unknown severity counts as an error, as editors show it.
    pub fn severity(&self) -> Severity {
        match self.severity {
            Some(2) => Severity::Warning,
            Some(3) => Severity::Info,
            Some(4) => Severity::Hint,
            _ => Severity::Error,
        }
    }
}

/// The `file://` URI for an absolute path; `None` for a relative one.
pub fn uri_for(path: &Path) -> Option<String> {
    url::Url::from_file_path(path).ok().map(String::from)
}

/// The path a `file://` URI names; `None` for any other scheme or a
/// malformed URI. Servers may percent-encode differently from cox, so a
/// URI is compared through this, never as a string.
pub fn path_for(uri: &str) -> Option<PathBuf> {
    url::Url::parse(uri).ok()?.to_file_path().ok()
}

/// One line per diagnostic, `path:line:col: severity: message [source code]`
/// with 1-based line and column and `path` relative to `root`, most severe
/// first and then by position, and a summary line last. The column counts
/// UTF-16 units as the server does; it matches characters on ASCII lines.
pub fn format(root: &Path, path: &Path, diags: &[Diagnostic]) -> String {
    let shown = path
        .strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string();
    let mut sorted: Vec<&Diagnostic> = diags.iter().collect();
    sorted.sort_by_key(|d| (d.severity(), d.range.start));
    let mut out = String::new();
    for d in &sorted {
        let start = d.range.start;
        // Servers send multi-line messages (rustc notes); one line each
        // keeps the `path:line:col` shape greppable.
        let message = d
            .message
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let _ = write!(
            out,
            "{shown}:{}:{}: {}: {message}",
            start.line + 1,
            start.character + 1,
            d.severity().label()
        );
        let tag: Vec<String> = d
            .source
            .iter()
            .cloned()
            .chain(d.code.as_ref().and_then(code_text))
            .collect();
        if !tag.is_empty() {
            let _ = write!(out, " [{}]", tag.join(" "));
        }
        out.push('\n');
    }
    out.push_str(&summary(&sorted));
    out
}

fn code_text(code: &Value) -> Option<String> {
    match code {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn summary(diags: &[&Diagnostic]) -> String {
    let parts: Vec<String> = [
        Severity::Error,
        Severity::Warning,
        Severity::Info,
        Severity::Hint,
    ]
    .into_iter()
    .filter_map(|sev| {
        let n = diags.iter().filter(|d| d.severity() == sev).count();
        let plural = if n == 1 || sev == Severity::Info {
            ""
        } else {
            "s"
        };
        (n > 0).then(|| format!("{n} {}{plural}", sev.label()))
    })
    .collect();
    if parts.is_empty() {
        "no diagnostics".to_string()
    } else {
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Trimmed from a rust-analyzer 2026 `publishDiagnostics` for a type
    /// error plus an unused variable, with the fields cox ignores left in.
    const RUST_ANALYZER_SAMPLE: &str = r#"{
      "uri": "file:///work/my%20crate/src/main.rs",
      "version": 3,
      "diagnostics": [
        {
          "range": {"start": {"line": 4, "character": 17}, "end": {"line": 4, "character": 19}},
          "severity": 1,
          "code": "E0308",
          "codeDescription": {"href": "https://doc.rust-lang.org/error-index.html#E0308"},
          "source": "rustc",
          "message": "mismatched types\nexpected `u32`, found `&str`",
          "relatedInformation": [],
          "data": {"rendered": "..."}
        },
        {
          "range": {"start": {"line": 2, "character": 8}, "end": {"line": 2, "character": 9}},
          "severity": 2,
          "code": "unused_variables",
          "source": "rustc",
          "message": "unused variable: `x`",
          "tags": [1]
        }
      ]
    }"#;

    fn diag(line: u32, character: u32, severity: Option<u8>, message: &str) -> Diagnostic {
        Diagnostic {
            range: Range {
                start: Position { line, character },
                end: Position { line, character },
            },
            severity,
            message: message.to_string(),
            ..Diagnostic::default()
        }
    }

    #[test]
    fn diagnostic_parses_from_rust_analyzer_sample() {
        let p: PublishDiagnosticsParams = serde_json::from_str(RUST_ANALYZER_SAMPLE).unwrap();
        assert_eq!(p.version, Some(3));
        assert_eq!(p.diagnostics.len(), 2);
        let first = &p.diagnostics[0];
        assert_eq!(first.severity(), Severity::Error);
        assert_eq!(first.code, Some(json!("E0308")));
        assert_eq!(first.source.as_deref(), Some("rustc"));
        assert_eq!(
            first.range.start,
            Position {
                line: 4,
                character: 17
            }
        );
        assert_eq!(
            path_for(&p.uri),
            Some(PathBuf::from("/work/my crate/src/main.rs"))
        );

        // Only a range and a message: everything optional defaults.
        let bare: Diagnostic = serde_json::from_value(json!({
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
            "message": "m",
            "code": 42
        }))
        .unwrap();
        assert_eq!(bare.severity, None);
        assert_eq!(bare.severity(), Severity::Error);
        assert_eq!(bare.source, None);
    }

    #[test]
    fn format_is_one_based_and_sorted() {
        let root = Path::new("/work");
        let path = Path::new("/work/src/a.rs");
        let mut tagged = diag(9, 0, Some(1), "late error\n  note: spans lines");
        tagged.source = Some("rustc".into());
        tagged.code = Some(json!("E0308"));
        let mut numbered = diag(0, 4, Some(1), "early error");
        numbered.code = Some(json!(7));
        let diags = [
            diag(0, 0, Some(2), "a warning"),
            tagged,
            diag(3, 2, Some(4), "a hint"),
            numbered,
            diag(1, 0, None, "no severity"),
        ];
        assert_eq!(
            format(root, path, &diags),
            "src/a.rs:1:5: error: early error [7]\n\
             src/a.rs:2:1: error: no severity\n\
             src/a.rs:10:1: error: late error note: spans lines [rustc E0308]\n\
             src/a.rs:1:1: warning: a warning\n\
             src/a.rs:4:3: hint: a hint\n\
             3 errors, 1 warning, 1 hint"
        );
        assert_eq!(format(root, path, &[]), "no diagnostics");
        // A path outside the root is printed as given.
        assert!(
            format(Path::new("/elsewhere"), path, &diags[..1]).starts_with("/work/src/a.rs:1:1")
        );
    }

    #[test]
    fn uri_round_trips_a_path_with_spaces() {
        let path = Path::new("/tmp/my project/src/lib #1.rs");
        let uri = uri_for(path).unwrap();
        assert!(uri.starts_with("file:///tmp/my%20project/"), "{uri}");
        assert!(!uri.contains(' '), "{uri}");
        assert_eq!(path_for(&uri).as_deref(), Some(path));
        assert_eq!(uri_for(Path::new("relative/x.rs")), None);
        assert_eq!(path_for("https://example.com/x.rs"), None);
    }
}
