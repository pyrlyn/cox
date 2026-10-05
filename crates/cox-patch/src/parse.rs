// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The V4A grammar: text → [`Patch`] and back. No I/O, no filesystem — a
//! patch that parses here has still not been checked against a single file,
//! which is what lets the fuzz target and the round-trip property run
//! without a workspace.
//!
//! Errors are `ToolError::Denied { why }` rather than a crate-local
//! `thiserror` enum: every one of them is a message the model reads and
//! retries against, the boundary would convert to exactly this variant
//! anyway, and a private enum here would need a new dependency
//! (`thiserror` is not a `cox-tools` dependency) to say the same thing.

use std::fmt;

use cox_protocol::ToolError;

const BEGIN: &str = "*** Begin Patch";
const END: &str = "*** End Patch";
const ADD: &str = "*** Add File: ";
const DELETE: &str = "*** Delete File: ";
const UPDATE: &str = "*** Update File: ";
const MOVE: &str = "*** Move to: ";
const EOF: &str = "*** End of File";

/// One line inside an update hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HunkLine {
    /// Context: present before and after, printed with a leading space.
    Keep(String),
    /// Removed by the patch.
    Del(String),
    /// Added by the patch.
    Add(String),
}

/// One `@@` block: the context path that locates it, then its lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// The `@@` headers, outermost first. A bare `@@` is the empty string,
    /// so this is never empty — a hunk cannot exist without its marker.
    pub context: Vec<String>,
    /// The hunk body, in file order.
    pub lines: Vec<HunkLine>,
    /// Whether the hunk ended with `*** End of File`, which anchors it to
    /// the end of the file rather than to the first context match.
    pub eof: bool,
}

/// One file operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Create `path` with exactly `lines`.
    Add {
        /// Workspace-relative path to create.
        path: String,
        /// The new file's lines, without the `+` prefix.
        lines: Vec<String>,
    },
    /// Remove `path`.
    Delete {
        /// Workspace-relative path to remove.
        path: String,
    },
    /// Rewrite `path` hunk by hunk, optionally renaming it to `move_to`.
    Update {
        /// Workspace-relative path to edit.
        path: String,
        /// Destination path when the patch also renames the file.
        move_to: Option<String>,
        /// The `@@` blocks, in file order.
        hunks: Vec<Hunk>,
    },
}

impl Op {
    /// The file this op reads from — the key it is staged under.
    pub fn path(&self) -> &str {
        match self {
            Op::Add { path, .. } | Op::Delete { path } | Op::Update { path, .. } => path,
        }
    }
}

/// A whole `*** Begin Patch` … `*** End Patch` document.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Patch {
    /// The file operations, in the order the patch lists them.
    pub ops: Vec<Op>,
}

impl Patch {
    /// How many files the patch removes. Drives the destructive-risk
    /// threshold in `cox_tools::v4a::tool` (T32.6 kept that threshold with
    /// the `Tool` impl, not in this crate).
    pub fn deletes(&self) -> usize {
        self.ops
            .iter()
            .filter(|op| matches!(op, Op::Delete { .. }))
            .count()
    }
}

/// Prints the patch back to V4A text. The inverse of [`parse`]: the
/// round-trip property in this module's tests is what keeps the two halves
/// from drifting apart.
impl fmt::Display for Patch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{BEGIN}")?;
        for op in &self.ops {
            match op {
                Op::Add { path, lines } => {
                    writeln!(f, "{ADD}{path}")?;
                    for l in lines {
                        writeln!(f, "+{l}")?;
                    }
                }
                Op::Delete { path } => writeln!(f, "{DELETE}{path}")?,
                Op::Update {
                    path,
                    move_to,
                    hunks,
                } => {
                    writeln!(f, "{UPDATE}{path}")?;
                    if let Some(to) = move_to {
                        writeln!(f, "{MOVE}{to}")?;
                    }
                    for h in hunks {
                        for c in &h.context {
                            if c.is_empty() {
                                writeln!(f, "@@")?;
                            } else {
                                writeln!(f, "@@ {c}")?;
                            }
                        }
                        for l in &h.lines {
                            match l {
                                HunkLine::Keep(t) => writeln!(f, " {t}")?,
                                HunkLine::Del(t) => writeln!(f, "-{t}")?,
                                HunkLine::Add(t) => writeln!(f, "+{t}")?,
                            }
                        }
                        if h.eof {
                            writeln!(f, "{EOF}")?;
                        }
                    }
                }
            }
        }
        write!(f, "{END}")
    }
}

/// `parse` failure. Carries the 1-based source line so a model that emitted
/// a malformed patch is told *where*, not just that it was rejected.
fn bad(line: usize, why: &str) -> ToolError {
    ToolError::Denied {
        why: format!("invalid patch at line {line}: {why}"),
    }
}

/// Parses V4A text. Anything before `*** Begin Patch` or after
/// `*** End Patch` is rejected rather than skipped: a patch buried in prose
/// is a sign the model wrapped it in commentary, and silently accepting
/// that trains the failure in.
pub fn parse(src: &str) -> Result<Patch, ToolError> {
    // `split('\n')`, not `str::lines`: `lines` also eats one trailing `\r`,
    // so a content line ending in `\r` lost one on every print/parse pass
    // (nightly fuzz, 2026-09-05). `apply` splits files the same way, and the
    // trailing-whitespace match level absorbs a CRLF patch against an LF file.
    let lines: Vec<&str> = src.split('\n').collect();
    let mut i = 0;
    // A trailing newline in the tool argument is not a syntax error.
    while lines.get(i).is_some_and(|l| l.trim().is_empty()) {
        i += 1;
    }
    if lines.get(i).map(|l| l.trim_end()) != Some(BEGIN) {
        return Err(bad(i + 1, "expected `*** Begin Patch`"));
    }
    i += 1;

    let mut ops = Vec::new();
    loop {
        let Some(line) = lines.get(i) else {
            return Err(bad(i + 1, "unterminated patch: expected `*** End Patch`"));
        };
        if line.trim_end() == END {
            i += 1;
            break;
        }
        if let Some(path) = line.strip_prefix(ADD) {
            i += 1;
            let mut body = Vec::new();
            while let Some(l) = lines.get(i) {
                let Some(text) = l.strip_prefix('+') else {
                    break;
                };
                body.push(text.to_string());
                i += 1;
            }
            ops.push(Op::Add {
                path: path.trim().to_string(),
                lines: body,
            });
        } else if let Some(path) = line.strip_prefix(DELETE) {
            i += 1;
            ops.push(Op::Delete {
                path: path.trim().to_string(),
            });
        } else if let Some(path) = line.strip_prefix(UPDATE) {
            i += 1;
            let mut move_to = None;
            if let Some(to) = lines.get(i).and_then(|l| l.strip_prefix(MOVE)) {
                move_to = Some(to.trim().to_string());
                i += 1;
            }
            let (hunks, next) = parse_hunks(&lines, i)?;
            if hunks.is_empty() {
                return Err(bad(i + 1, "`*** Update File` with no `@@` hunk"));
            }
            i = next;
            ops.push(Op::Update {
                path: path.trim().to_string(),
                move_to,
                hunks,
            });
        } else {
            return Err(bad(i + 1, "expected `*** Add/Delete/Update File:`"));
        }
    }
    if let Some(extra) = lines[i..].iter().position(|l| !l.trim().is_empty()) {
        return Err(bad(i + extra + 1, "trailing text after `*** End Patch`"));
    }
    Ok(Patch { ops })
}

/// Reads consecutive `@@` blocks starting at `start`, returning them and the
/// index of the first line that is not part of one.
fn parse_hunks(lines: &[&str], start: usize) -> Result<(Vec<Hunk>, usize), ToolError> {
    let mut i = start;
    let mut hunks: Vec<Hunk> = Vec::new();
    while let Some(line) = lines.get(i) {
        if !line.starts_with("@@") {
            break;
        }
        let mut context = Vec::new();
        while let Some(rest) = lines.get(i).and_then(|l| l.strip_prefix("@@")) {
            context.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
            i += 1;
        }
        let mut body = Vec::new();
        let mut eof = false;
        while let Some(l) = lines.get(i) {
            if l.trim_end() == EOF {
                eof = true;
                i += 1;
                break;
            }
            if l.starts_with("@@") || l.starts_with("*** ") || l.trim_end() == END {
                break;
            }
            // An entirely empty line is a context line whose emitter dropped
            // the trailing space; treating it as a syntax error would reject
            // patches every real model produces.
            body.push(match l.chars().next() {
                Some('-') => HunkLine::Del(l[1..].to_string()),
                Some('+') => HunkLine::Add(l[1..].to_string()),
                Some(' ') => HunkLine::Keep(l[1..].to_string()),
                None => HunkLine::Keep(String::new()),
                Some(_) => return Err(bad(i + 1, "hunk line must start with ` `, `-` or `+`")),
            });
            i += 1;
        }
        if body.is_empty() {
            return Err(bad(i + 1, "empty `@@` hunk"));
        }
        hunks.push(Hunk {
            context,
            lines: body,
            eof,
        });
    }
    Ok((hunks, i))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn p(src: &str) -> Patch {
        parse(src).expect("parse")
    }

    #[test]
    fn v4a_parses_codex_documented_shape() {
        let patch = p("*** Begin Patch\n\
             *** Add File: new.py\n\
             +print(1)\n\
             *** Update File: old.py\n\
             *** Move to: moved.py\n\
             @@ class Foo\n\
             @@     def bar():\n\
             -        return 1\n\
             +        return 2\n\
             *** Delete File: gone.py\n\
             *** End Patch");
        assert_eq!(patch.ops.len(), 3);
        assert_eq!(
            patch.ops[0],
            Op::Add {
                path: "new.py".into(),
                lines: vec!["print(1)".into()],
            }
        );
        let Op::Update { move_to, hunks, .. } = &patch.ops[1] else {
            panic!("expected an update, got {:?}", patch.ops[1]);
        };
        assert_eq!(move_to.as_deref(), Some("moved.py"));
        assert_eq!(hunks[0].context, vec!["class Foo", "    def bar():"]);
        assert_eq!(patch.deletes(), 1);
    }

    #[test]
    fn v4a_bare_at_marker_and_end_of_file_survive() {
        let patch =
            p("*** Begin Patch\n*** Update File: a\n@@\n x\n+y\n*** End of File\n*** End Patch");
        let Op::Update { hunks, .. } = &patch.ops[0] else {
            panic!("expected an update");
        };
        assert_eq!(hunks[0].context, vec![""]);
        assert!(hunks[0].eof, "`*** End of File` must survive the parse");
    }

    #[test]
    fn v4a_trailing_carriage_returns_survive_the_round_trip() {
        let src = "*** Begin Patch\n*** Add File: a\n+x\r\r\n*** Update File: b\n@@\n y\r\n*** End Patch\r\n";
        let patch = p(src);
        assert_eq!(
            patch.ops[0],
            Op::Add {
                path: "a".into(),
                lines: vec!["x\r\r".into()],
            }
        );
        assert_eq!(parse(&patch.to_string()).ok(), Some(patch));
    }

    #[test]
    fn v4a_rejects_prose_around_the_patch() {
        for src in [
            "here you go:\n*** Begin Patch\n*** Delete File: a\n*** End Patch",
            "*** Begin Patch\n*** Delete File: a\n*** End Patch\nhope that helps",
            "*** Begin Patch\n*** Delete File: a",
            "*** Begin Patch\n*** Update File: a\n@@\nnot a hunk line\n*** End Patch",
            "*** Begin Patch\n*** Update File: a\n*** End Patch",
        ] {
            assert!(parse(src).is_err(), "should have been rejected: {src:?}");
        }
    }

    /// Text that cannot be confused with a marker: every generated line is
    /// emitted behind a ` `/`-`/`+` prefix, so the alphabet only has to
    /// exclude newlines to keep the grammar unambiguous.
    fn text() -> impl Strategy<Value = String> {
        "[a-z @*+-]{0,12}"
    }

    fn hunk() -> impl Strategy<Value = Hunk> {
        (
            prop::collection::vec(text(), 1..3),
            prop::collection::vec(
                prop_oneof![
                    text().prop_map(HunkLine::Keep),
                    text().prop_map(HunkLine::Del),
                    text().prop_map(HunkLine::Add),
                ],
                1..6,
            ),
            any::<bool>(),
        )
            .prop_map(|(context, lines, eof)| Hunk {
                context,
                lines,
                eof,
            })
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            ("[a-z/]{1,8}", prop::collection::vec(text(), 0..4))
                .prop_map(|(path, lines)| Op::Add { path, lines }),
            "[a-z/]{1,8}".prop_map(|path| Op::Delete { path }),
            (
                "[a-z/]{1,8}",
                prop::option::of("[a-z/]{1,8}"),
                prop::collection::vec(hunk(), 1..3),
            )
                .prop_map(|(path, move_to, hunks)| Op::Update {
                    path,
                    move_to,
                    hunks,
                }),
        ]
    }

    proptest! {
        /// The grammar is a bijection on well-formed patches. This is the
        /// property the fuzz target (`fuzz/fuzz_targets/v4a_parse.rs`)
        /// attacks from the other side, with arbitrary bytes.
        #[test]
        fn v4a_parse_of_print_is_identity(patch in prop::collection::vec(op(), 0..4).prop_map(|ops| Patch { ops })) {
            let printed = patch.to_string();
            let reparsed = parse(&printed).map_err(|e| TestCaseError::fail(format!("{e} in:\n{printed}")))?;
            prop_assert_eq!(reparsed, patch);
        }

        /// The same claim from the other side, and the one the fuzz target
        /// asserts: text `parse` *accepts* must print back to text it
        /// accepts identically. Without this, cox echoing a patch back
        /// (rollout replay, a retry) could change what it applies.
        #[test]
        fn v4a_printing_an_accepted_patch_is_idempotent(
            src in prop::collection::vec(
                prop_oneof![
                    Just("*** Begin Patch".to_string()),
                    Just("*** End Patch".to_string()),
                    Just("*** End of File".to_string()),
                    "\\*\\*\\* (Add|Delete|Update) File: [a-z ]{0,4}",
                    "\\*\\*\\* Move to: [a-z ]{0,4}",
                    "@@[ a-z]{0,4}",
                    "[ +-][a-z@*\\r ]{0,4}",
                    "[a-z*@\\r ]{0,4}",
                ],
                0..8,
            ).prop_map(|l| l.join("\n")),
        ) {
            if let Ok(patch) = parse(&src) {
                let printed = patch.to_string();
                prop_assert_eq!(parse(&printed).ok(), Some(patch), "printed:\n{}", printed);
            }
        }
    }
}
