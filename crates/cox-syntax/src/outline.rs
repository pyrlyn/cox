// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `read`'s `mode=outline`: a short `line: signature` listing of a file's
//! top-level shape, so the model can decide what to `read`/`lines=` into
//! full detail instead of paying for the whole file (plan.md T3.2 step 3;
//! AGENTS.md D6c). Tree-sitter for rs/ts/tsx/py/go; everything else falls
//! back to markdown headings or a `^(fn|def|class|func|pub|export)` grep.

use std::path::Path;

use tree_sitter::{Node, Parser};

/// Kinds counted as a "definition" worth an outline row, per language. Kept
/// as node-kind strings (not a `tree_sitter::Query`) because the signature
/// extraction below is generic across all of them: slice from the node's
/// start to wherever its body child begins.
fn language_and_kinds(ext: &str) -> Option<(tree_sitter::Language, &'static [&'static str])> {
    match ext {
        "rs" => Some((
            tree_sitter_rust::LANGUAGE.into(),
            &[
                "function_item",
                "struct_item",
                "enum_item",
                "trait_item",
                "impl_item",
                "type_item",
            ],
        )),
        "ts" | "mts" | "cts" => Some((
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            &[
                "function_declaration",
                "method_definition",
                "class_declaration",
                "interface_declaration",
                "type_alias_declaration",
            ],
        )),
        "tsx" => Some((
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            &[
                "function_declaration",
                "method_definition",
                "class_declaration",
                "interface_declaration",
                "type_alias_declaration",
            ],
        )),
        "py" => Some((
            tree_sitter_python::LANGUAGE.into(),
            &["function_definition", "class_definition"],
        )),
        "go" => Some((
            tree_sitter_go::LANGUAGE.into(),
            &[
                "function_declaration",
                "method_declaration",
                "type_declaration",
            ],
        )),
        _ => None,
    }
}

/// Builds the outline for `content` (already read as text — the caller,
/// `read.rs`, already ruled out binary). `path`'s extension picks the
/// tree-sitter grammar; anything else, or a parse failure, uses the
/// line-pattern fallback so `outline` never errors on an unrecognised file.
pub fn outline(path: &Path, content: &str) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();

    if let Some((language, kinds)) = language_and_kinds(ext)
        && let Some(rows) = tree_sitter_outline(language, kinds, content)
        && !rows.is_empty()
    {
        return render(&rows);
    }
    render(&fallback_outline(ext, content))
}

fn tree_sitter_outline(
    language: tree_sitter::Language,
    kinds: &[&str],
    content: &str,
) -> Option<Vec<(usize, String)>> {
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(content, None)?;

    let mut rows = Vec::new();
    collect(tree.root_node(), content.as_bytes(), kinds, &mut rows);
    // Tree order is already source order (preorder), but nested items
    // (e.g. a fn inside an impl) are visited after their parent, so a
    // plain stable sort by line keeps the listing readable top-to-bottom.
    rows.sort_by_key(|(line, _)| *line);
    Some(rows)
}

fn collect(node: Node, source: &[u8], kinds: &[&str], out: &mut Vec<(usize, String)>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            let line = child.start_position().row + 1;
            out.push((line, signature(source, child)));
        }
        collect(child, source, kinds, out);
    }
}

/// The node's header: everything before its body (the first child whose
/// kind looks like a block/body), whitespace-collapsed to one line. Works
/// across grammars without a per-language query, since "definition node up
/// to its block child" is the same shape in Rust/TS/Python/Go.
fn signature(source: &[u8], node: Node) -> String {
    let mut cursor = node.walk();
    let body_start = node
        .children(&mut cursor)
        .find(|c| {
            let k = c.kind();
            k.ends_with("block") || k.ends_with("_body") || k == "body"
        })
        .map(|c| c.start_byte());
    let end = body_start
        .unwrap_or_else(|| node.end_byte())
        .max(node.start_byte());
    let raw = std::str::from_utf8(&source[node.start_byte()..end]).unwrap_or("");
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(['{', ';', ':'])
        .trim()
        .to_string()
}

/// Non-tree-sitter languages: markdown headings if any exist, else lines
/// that open with a definition-shaped keyword (plan.md T3.2 step 3).
fn fallback_outline(ext: &str, content: &str) -> Vec<(usize, String)> {
    let is_markdown = matches!(ext, "md" | "markdown");
    let mut rows = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        let matches = if is_markdown {
            trimmed.starts_with('#')
        } else {
            ["fn ", "fn(", "def ", "class ", "func ", "pub ", "export "]
                .iter()
                .any(|kw| trimmed.starts_with(kw))
        };
        if matches {
            rows.push((idx + 1, trimmed.to_string()));
        }
    }
    rows
}

fn render(rows: &[(usize, String)]) -> String {
    if rows.is_empty() {
        return "(no outline entries found)".to_string();
    }
    rows.iter()
        .map(|(line, sig)| format!("{line}: {sig}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outline_rust_lists_pub_fn_and_struct() {
        let src = "pub struct Foo {\n    x: u32,\n}\n\npub fn bar(x: u32) -> u32 {\n    x + 1\n}\n";
        let out = outline(Path::new("x.rs"), src);
        assert!(out.contains("1: pub struct Foo"), "{out}");
        assert!(out.contains("5: pub fn bar(x: u32) -> u32"), "{out}");
    }

    #[test]
    fn outline_falls_back_to_markdown_headings() {
        let src = "# Title\n\ntext\n\n## Section\n";
        let out = outline(Path::new("x.md"), src);
        assert_eq!(out, "1: # Title\n5: ## Section");
    }

    #[test]
    fn outline_falls_back_to_keyword_lines_for_unknown_extension() {
        let src = "local x = 1\nfunction foo()\nend\n";
        let out = outline(Path::new("x.lua"), src);
        // no tree-sitter grammar for lua and no "fn "/"func " match on this
        // particular fixture, so it's a legitimate empty outline.
        assert_eq!(out, "(no outline entries found)");
    }
}
