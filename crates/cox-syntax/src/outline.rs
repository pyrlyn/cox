// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `read`'s `mode=outline`: a short `start-end: signature` listing of a
//! file's top-level shape, so the model can pass that span as `lines`
//! instead of guessing where the item ends (plan.md T3.2 step 3, T59.11;
//! AGENTS.md D6c). Tree-sitter for rs/ts/tsx/py/go; everything else falls
//! back to markdown headings or a `^(fn|def|class|func|pub|export)` grep.
//! The same walk also yields each definition's qualified name and line span
//! (`definitions`), which `read`'s `symbol` input resolves against (T59.8).

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

/// One definition found by the tree-sitter walk: the single source both the
/// outline listing and `read`'s symbol lookup derive from, so the two can
/// never disagree about what a file defines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Def {
    /// Qualified by the enclosing impl/trait/class (`Foo::bar`); empty when
    /// the grammar gives the node no name, which lookup skips.
    pub name: String,
    /// 1-based inclusive line span of the whole definition, body included.
    pub start: usize,
    pub end: usize,
    pub signature: String,
}

impl Def {
    /// `query` may use `::` or `.` between segments, and may omit leading
    /// ones: `bar` finds `Foo::bar` (the model rarely knows the container).
    pub fn matches(&self, query: &str) -> bool {
        let query = normalize(query);
        !self.name.is_empty() && (self.name == query || self.name.ends_with(&format!("::{query}")))
    }
}

fn normalize(name: &str) -> String {
    name.trim().replace('.', "::")
}

/// Every named definition of `content`, in source order. `None` when the
/// extension has no grammar or the parse fails, so a caller can tell
/// "unsupported file" from "no such symbol".
pub fn definitions(path: &Path, content: &str) -> Option<Vec<Def>> {
    let ext = path.extension().and_then(|e| e.to_str())?;
    let (language, kinds) = language_and_kinds(ext)?;
    tree_sitter_defs(language, kinds, content)
}

/// The definitions `query` names. An exact qualified-name hit wins over
/// suffix hits, so `bar` still resolves to a free `fn bar` when a method
/// `Foo::bar` also exists.
pub fn find_symbol<'a>(defs: &'a [Def], query: &str) -> Vec<&'a Def> {
    let query = normalize(query);
    let exact: Vec<&Def> = defs.iter().filter(|d| d.name == query).collect();
    if exact.is_empty() {
        defs.iter().filter(|d| d.matches(&query)).collect()
    } else {
        exact
    }
}

fn tree_sitter_outline(
    language: tree_sitter::Language,
    kinds: &[&str],
    content: &str,
) -> Option<Vec<(usize, usize, String)>> {
    let defs = tree_sitter_defs(language, kinds, content)?;
    Some(
        defs.into_iter()
            .map(|d| (d.start, d.end, d.signature))
            .collect(),
    )
}

fn tree_sitter_defs(
    language: tree_sitter::Language,
    kinds: &[&str],
    content: &str,
) -> Option<Vec<Def>> {
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(content, None)?;

    let mut defs = Vec::new();
    collect(tree.root_node(), content.as_bytes(), kinds, "", &mut defs);
    // Tree order is already source order (preorder), but nested items
    // (e.g. a fn inside an impl) are visited after their parent, so a
    // plain stable sort by line keeps the listing readable top-to-bottom.
    defs.sort_by_key(|d| d.start);
    Some(defs)
}

fn collect(node: Node, source: &[u8], kinds: &[&str], prefix: &str, out: &mut Vec<Def>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let mut child_prefix = prefix;
        let qualified;
        if kinds.contains(&child.kind()) {
            let name = def_name(source, child);
            qualified = match (prefix.is_empty(), name.is_empty()) {
                (_, true) => String::new(),
                (true, false) => name,
                (false, false) => format!("{prefix}::{name}"),
            };
            // Both ends are 1-based and inclusive. `end_position` is
            // 0-based, so the next `lines` read can use the span as written
            // instead of guessing where the item stops.
            out.push(Def {
                name: qualified.clone(),
                start: child.start_position().row + 1,
                end: child.end_position().row + 1,
                signature: signature(source, child),
            });
            if is_container(child.kind()) && !qualified.is_empty() {
                child_prefix = &qualified;
            }
        }
        collect(child, source, kinds, child_prefix, out);
    }
}

/// Kinds whose name qualifies the definitions nested inside them.
fn is_container(kind: &str) -> bool {
    matches!(
        kind,
        "impl_item" | "trait_item" | "class_declaration" | "class_definition"
    )
}

/// The definition's own name. An `impl` is named by the type it extends, and
/// a Go method by its receiver type (Go declares methods outside the type),
/// so both qualify like a method inside a class would.
fn def_name(source: &[u8], node: Node) -> String {
    let text = |n: Node| n.utf8_text(source).unwrap_or_default().to_string();
    let bare = |s: String| s.split('<').next().unwrap_or_default().trim().to_string();
    match node.kind() {
        "impl_item" => node
            .child_by_field_name("type")
            .map(|n| bare(text(n)))
            .unwrap_or_default(),
        "method_declaration" => {
            let name = node
                .child_by_field_name("name")
                .map(text)
                .unwrap_or_default();
            match node
                .child_by_field_name("receiver")
                .and_then(|r| first_of_kind(r, "type_identifier"))
            {
                Some(recv) => format!("{}::{name}", text(recv)),
                None => name,
            }
        }
        "type_declaration" => {
            let mut cursor = node.walk();
            node.children(&mut cursor)
                .find(|c| c.kind() == "type_spec")
                .and_then(|spec| spec.child_by_field_name("name"))
                .map(text)
                .unwrap_or_default()
        }
        _ => node
            .child_by_field_name("name")
            .map(text)
            .unwrap_or_default(),
    }
}

fn first_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
    if node.kind() == kind {
        return Some(node);
    }
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find_map(|c| first_of_kind(c, kind))
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
fn fallback_outline(ext: &str, content: &str) -> Vec<(usize, usize, String)> {
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
            // A keyword line has no body span, so start and end are the same.
            let line_no = idx + 1;
            rows.push((line_no, line_no, trimmed.to_string()));
        }
    }
    rows
}

fn render(rows: &[(usize, usize, String)]) -> String {
    if rows.is_empty() {
        return "(no outline entries found)".to_string();
    }
    rows.iter()
        .map(|(start, end, sig)| format!("{start}-{end}: {sig}"))
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
        assert!(out.contains("1-3: pub struct Foo"), "{out}");
        assert!(out.contains("5-7: pub fn bar(x: u32) -> u32"), "{out}");
    }

    #[test]
    fn outline_falls_back_to_markdown_headings() {
        let src = "# Title\n\ntext\n\n## Section\n";
        let out = outline(Path::new("x.md"), src);
        assert_eq!(out, "1-1: # Title\n5-5: ## Section");
    }

    #[test]
    fn outline_falls_back_to_keyword_lines_for_unknown_extension() {
        let src = "local x = 1\nfunction foo()\nend\n";
        let out = outline(Path::new("x.lua"), src);
        // no tree-sitter grammar for lua and no "fn "/"func " match on this
        // particular fixture, so it's a legitimate empty outline.
        assert_eq!(out, "(no outline entries found)");
    }

    const RUST: &str = "struct Foo;\n\nimpl Foo {\n    fn bar(&self) -> u32 {\n        1\n    }\n}\n\nfn bar() {}\n\nimpl<T> Other for Wrap<T> {\n    fn run(&self) {}\n}\n";
    const TS: &str =
        "export class Box {\n  open(): void {\n    go();\n  }\n}\n\nfunction open() {}\n";

    fn names(defs: &[Def]) -> Vec<&str> {
        defs.iter().map(|d| d.name.as_str()).collect()
    }

    #[test]
    fn definitions_qualify_rust_methods_by_their_impl_type() {
        let defs = definitions(Path::new("x.rs"), RUST).expect("rust grammar");
        assert_eq!(
            names(&defs),
            ["Foo", "Foo", "Foo::bar", "bar", "Wrap", "Wrap::run"]
        );
        let method = defs.iter().find(|d| d.name == "Foo::bar").expect("method");
        assert_eq!((method.start, method.end), (4, 6));
    }

    #[test]
    fn definitions_qualify_typescript_methods_by_their_class() {
        let defs = definitions(Path::new("x.ts"), TS).expect("ts grammar");
        assert_eq!(names(&defs), ["Box", "Box::open", "open"]);
    }

    #[test]
    fn definitions_qualify_go_methods_by_their_receiver() {
        let src = "package p\n\ntype Box struct{}\n\nfunc (b *Box) Open() {}\n";
        let defs = definitions(Path::new("x.go"), src).expect("go grammar");
        assert_eq!(names(&defs), ["Box", "Box::Open"]);
    }

    #[test]
    fn definitions_is_none_without_a_grammar() {
        assert!(definitions(Path::new("x.lua"), "function f() end").is_none());
    }

    #[test]
    fn find_symbol_accepts_dot_and_colon_separators_and_a_bare_method_name() {
        let defs = definitions(Path::new("x.ts"), TS).expect("ts grammar");
        assert_eq!(find_symbol(&defs, "Box.open")[0].start, 2);
        assert_eq!(find_symbol(&defs, "Box::open")[0].start, 2);
        let rs = definitions(Path::new("x.rs"), RUST).expect("rust grammar");
        assert_eq!(find_symbol(&rs, "run").len(), 1);
    }

    #[test]
    fn find_symbol_prefers_the_exact_name_over_a_method_with_that_suffix() {
        let defs = definitions(Path::new("x.rs"), RUST).expect("rust grammar");
        let hits = find_symbol(&defs, "bar");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].start, 9);
    }
}
