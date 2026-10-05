// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! A49 (plan.md T30.28): no test may read or write the real OS keychain.
//! Walks every crate's `src/` and `tests/` directories (plus a top-level
//! `tests/` if one exists) and fails if a `#[cfg(test)] mod .. { .. }` block,
//! or any file entirely under a `tests/` directory, still touches the real
//! keyring. A test injects the lookup instead
//! (`cox_provider::http::resolve_key_with`, a fake `cox_mcp::auth::Secrets`)
//! or sets its own env var so the env branch always wins before the keyring
//! fallback is ever consulted (`AGENTS.md` Conventions).
//!
//! Comments and string contents are blanked before the search, so a doc
//! comment or a string fixture that merely *mentions* one of these names —
//! including the ones this file's own tests plant — never counts as a call.

use std::fs;
use std::path::{Path, PathBuf};

/// Literal source patterns that touch the real platform keyring. Checked
/// against comment- and string-stripped text, so only an actual call
/// matches (`resolve_key_with(` is not a match: an underscore, not `(`,
/// follows `resolve_key`).
const FORBIDDEN: &[&str] = &["resolve_key(", "platform_keyring", "keyring::Entry"];

fn workspace_root() -> PathBuf {
    // This crate lives at `crates/cox`; the workspace root is two levels up.
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every `.rs` file under `dir`, recursively. A crate with no `tests/`
/// directory (or a workspace with no top-level one) just yields nothing.
fn rust_files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(rust_files_under(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    out
}

/// Replaces every `//`/`/* */` comment and the contents of `"..."` string
/// literals with spaces, keeping line breaks in place. Braces and forbidden
/// text inside either can then never affect matching or region-finding.
/// Raw strings (`r#"..."#`) are not special-cased — none of this workspace's
/// test code hides a keyring call inside one, and the worst a mismatch does
/// is widen or shrink a masked span, never silently drop a real call outside
/// any string.
fn mask(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                out.push_str("  ");
                i += 2;
                while i < bytes.len() {
                    if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        out.push_str("  ");
                        i += 2;
                        break;
                    }
                    out.push(if bytes[i] == b'\n' { '\n' } else { ' ' });
                    i += 1;
                }
            }
            b'"' => {
                out.push(' ');
                i += 1;
                while i < bytes.len() {
                    let b = bytes[i];
                    if b == b'\\' && i + 1 < bytes.len() {
                        out.push_str("  ");
                        i += 2;
                        continue;
                    }
                    out.push(if b == b'\n' { '\n' } else { ' ' });
                    i += 1;
                    if b == b'"' {
                        break;
                    }
                }
            }
            b => {
                out.push(b as char);
                i += 1;
            }
        }
    }
    out
}

/// The byte span of the matching `}` for the `{` at `masked[open]`.
fn matching_brace(masked: &str, open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (i, b) in masked.as_bytes().iter().enumerate().skip(open) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Test regions in already-`mask`ed source: the whole text when
/// `whole_file` is true (every file under a `tests/` directory is test-only
/// code), otherwise the span of every `#[cfg(test)] mod <name> { .. }`
/// block — the shape every test in this workspace follows (`AGENTS.md`
/// Conventions). A `#[cfg(test)]` item that is not a `mod` (a rare env-lock
/// helper, e.g. the `config_load::temp_env` re-export) is skipped rather than guessed at.
fn test_regions(masked: &str, whole_file: bool) -> Vec<(usize, usize)> {
    if whole_file {
        return vec![(0, masked.len())];
    }
    let bytes = masked.as_bytes();
    let marker = "#[cfg(test)]";
    let mut regions = Vec::new();
    let mut from = 0;
    while let Some(rel) = masked[from..].find(marker) {
        let attr_end = from + rel + marker.len();
        let mut j = attr_end;
        loop {
            while bytes.get(j).is_some_and(u8::is_ascii_whitespace) {
                j += 1;
            }
            if bytes.get(j) == Some(&b'#')
                && let Some(close) = masked[j..].find(']')
            {
                j += close + 1;
                continue;
            }
            break;
        }
        if masked[j..].starts_with("mod ")
            && let Some(brace_rel) = masked[j..].find('{')
            && let Some(end) = matching_brace(masked, j + brace_rel)
        {
            regions.push((j + brace_rel, end + 1));
            from = end + 1;
            continue;
        }
        from = attr_end;
    }
    regions
}

/// Every forbidden match inside `source`'s test regions, as
/// `"line N: `pattern`"` strings.
fn violations_in(source: &str, whole_file: bool) -> Vec<String> {
    let masked = mask(source);
    let mut hits = Vec::new();
    for (start, end) in test_regions(&masked, whole_file) {
        let region = &masked[start..end];
        for pattern in FORBIDDEN {
            let mut search_from = 0;
            while let Some(rel) = region[search_from..].find(pattern) {
                let pos = start + search_from + rel;
                let line = masked[..pos].matches('\n').count() + 1;
                hits.push(format!("line {line}: `{pattern}`"));
                search_from = search_from + rel + pattern.len();
            }
        }
    }
    hits
}

#[test]
fn no_test_reads_the_real_keychain() {
    let root = workspace_root();
    let mut violations = Vec::new();

    let mut scan = |file: &Path, whole_file: bool| {
        let Ok(src) = fs::read_to_string(file) else {
            return;
        };
        for hit in violations_in(&src, whole_file) {
            violations.push(format!("{}: {hit}", file.display()));
        }
    };

    if let Ok(entries) = fs::read_dir(root.join("crates")) {
        for entry in entries.flatten() {
            let crate_dir = entry.path();
            if !crate_dir.is_dir() {
                continue;
            }
            for file in rust_files_under(&crate_dir.join("src")) {
                scan(&file, false);
            }
            for file in rust_files_under(&crate_dir.join("tests")) {
                scan(&file, true);
            }
        }
    }
    for file in rust_files_under(&root.join("tests")) {
        scan(&file, true);
    }

    assert!(
        violations.is_empty(),
        "a test still touches the real OS keychain (A49, plan.md T30.28) — \
         inject the lookup instead:\n{}",
        violations.join("\n")
    );
}

#[test]
fn scanner_flags_a_planted_keyring_call_in_a_tests_module() {
    let planted = "#[cfg(test)]\nmod tests {\n    fn x() {\n        let _ = resolve_key(\"E\", \"s\");\n    }\n}\n";
    let hits = violations_in(planted, false);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(hits[0].contains("resolve_key("), "{hits:?}");
    assert!(hits[0].contains("line 4"), "{hits:?}");
}

#[test]
fn scanner_ignores_resolve_key_with_and_comment_mentions() {
    let clean = "#[cfg(test)]\nmod tests {\n    \
        // mentions platform_keyring and keyring::Entry in prose only\n    \
        fn x() {\n        let _ = resolve_key_with(\"E\", \"s\", |_| None);\n    }\n}\n";
    assert!(violations_in(clean, false).is_empty());
}

#[test]
fn scanner_treats_a_whole_tests_dir_file_as_test_code_with_no_cfg_attribute() {
    let planted = "fn helper() {\n    let _ = keyring::Entry::new(\"cox\", \"x\");\n}\n";
    assert!(
        violations_in(planted, false).is_empty(),
        "not inside a mod tests block"
    );
    assert_eq!(
        violations_in(planted, true).len(),
        1,
        "whole file is test code"
    );
}
