// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `classify(command) -> Risk` and `segments(command) -> Segments` for
//! `bash` (plan.md T3.7 step 3, T36.1, T36.2): one tree-sitter-bash walk
//! that splits the line on `;`, `&&`, `||`, pipes, `&` and newlines, keeps
//! the riskiest segment and lists every simple command for the permission
//! engine to match one by one. T36.2 closes three gaps in that walk: a
//! variable-assignment prefix or `export`/`declare`/`unset` line no longer
//! stays `ReadOnly` (unless it only touches a short list of variables that
//! cannot change what runs); a deny/ask rule sees a wrapped command
//! (`nohup`, `timeout 5`, …) unwrapped, not just as written; and it looks
//! inside an `eval`/`sh -c`/`bash -c` string by re-parsing it with the same
//! walk. Separate from the runner because the permission engine rates a
//! command line before anything runs, and tests drive it without a PTY.
//! Parser setup (`parse_bash`) lives in `cox-syntax` (T32.4: tree-sitter is
//! the only reason that crate exists); this file keeps the risk walk
//! itself, since it is domain logic, not parsing.

use cox_protocol::{Risk, Segments};
use cox_syntax::Node;

/// Commands that cannot change anything cox does not already show the model.
const READ_ONLY: &[&str] = &[
    "ls",
    "cat",
    "head",
    "tail",
    "grep",
    "rg",
    "egrep",
    "fgrep",
    "pwd",
    "echo",
    "printf",
    "wc",
    "which",
    "whereis",
    "type",
    "stat",
    "file",
    "tree",
    "diff",
    "sort",
    "uniq",
    "cut",
    "tr",
    "basename",
    "dirname",
    "realpath",
    "readlink",
    "date",
    "whoami",
    "id",
    "uname",
    "printenv",
    "cd",
    "true",
    "false",
    "test",
    "[",
    "du",
    "df",
    "ps",
    "jq",
    "less",
    "more",
    "md5sum",
    "sha256sum",
    "shasum",
    "hexdump",
    "xxd",
    "strings",
    "column",
    "nl",
    "fold",
    "paste",
    "comm",
    "tac",
    "rev",
    "seq",
    "expr",
];
/// Prefix commands whose risk is that of the command they run — the fixed
/// set Claude Code strips before matching a Bash rule (`timeout`, `time`,
/// `nice`, `nohup`, `stdbuf`, the builtins `command`/`builtin`, zsh's
/// `noglob`, and bare `xargs`; research.md row 38), plus `env` and `exec`,
/// cox's own extensions from T3.7. `xargs` and `timeout` need special
/// handling (`wrapper_args`) because their own arguments are not all
/// flags.
const WRAPPERS: &[&str] = &[
    "env", "command", "builtin", "noglob", "nohup", "time", "nice", "timeout", "stdbuf", "xargs",
    "exec",
];
const DOWNLOADERS: &[&str] = &["curl", "wget"];
const INTERPRETERS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "ksh", "python", "python3", "perl", "ruby", "node",
];
/// Shells whose `-c` runs a string the split cannot see into on its own —
/// `inner_code` recovers it by re-parsing the literal text (T36.2).
const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "fish", "csh", "tcsh"];
/// Redirect targets under `/dev/` that discard or echo rather than overwrite a device.
const HARMLESS_DEVICES: &[&str] = &["/dev/null", "/dev/stdout", "/dev/stderr", "/dev/tty"];
/// Environment variables whose value cannot change what a later command
/// resolves to or how it behaves: pure locale/display knobs, nothing a
/// program's own logic branches on for *what* it does. A leading
/// assignment of only these stays `ReadOnly` and un-opaque, same as no
/// assignment at all. Claude Code strips a leading assignment of "known-safe"
/// environment variables the same way, but its docs (research.md row 38,
/// checked 2026-09-26) name only one example, `NODE_ENV`, and do not
/// publish the full list — cox keeps its own, deliberately narrower list
/// instead of guessing at parity; `export`/`declare`/`unset` get no such
/// exception (below), since unsetting or re-exporting is a different,
/// wider act than a plain prefix.
const SAFE_ASSIGNMENTS: &[&str] = &["LC_ALL", "LANG", "TZ", "NO_COLOR"];
/// How many nested `eval`/`sh -c` strings `segments` re-parses before it
/// stops descending — bounded so a pathological chain of quoted scripts
/// cannot make classification loop or blow the stack; each level still
/// leaves the call opaque and at least `Exec`.
const MAX_INNER_DEPTH: u8 = 4;

/// The riskiest thing `command` can do, or `Exec` when it cannot be parsed.
pub fn classify(command: &str) -> Risk {
    scan_at(command, 0).0
}

/// The simple commands `command` runs, for the permission engine (T36.1,
/// T36.2). Opaque when the parse cannot vouch for the whole line: a parse
/// error, no command at all, a substitution, `eval`/`sh -c`, a variable
/// assignment other than a known-safe one (`PATH=… git` runs a different
/// `git`), `export`/`declare`/`unset`, or an output redirect to a path.
pub fn segments(command: &str) -> Segments {
    scan_at(command, 0).1
}

fn scan_at(command: &str, depth: u8) -> (Risk, Segments) {
    let opaque = Segments {
        commands: Vec::new(),
        opaque: true,
    };
    let Some(tree) = cox_syntax::parse_bash(command) else {
        return (Risk::Exec, opaque);
    };
    let broken = tree.root_node().has_error() || command.trim().is_empty();
    let mut scan = Scan {
        risk: if broken { Risk::Exec } else { Risk::ReadOnly },
        segments: Segments {
            opaque: broken,
            ..Segments::default()
        },
        depth,
    };
    walk(tree.root_node(), command.as_bytes(), &mut scan);
    if scan.segments.commands.is_empty() {
        scan.segments.opaque = true;
    }
    (scan.risk, scan.segments)
}

/// What one walk collects: the call's risk and its permission segments,
/// plus how many `eval`/`sh -c` strings deep this walk already is.
struct Scan {
    risk: Risk,
    segments: Segments,
    depth: u8,
}

fn rank(r: Risk) -> u8 {
    match r {
        Risk::ReadOnly => 0,
        Risk::Write => 1,
        Risk::Exec => 2,
        Risk::Destructive => 3,
    }
}

fn bump(cur: &mut Risk, r: Risk) {
    if rank(r) > rank(*cur) {
        *cur = r;
    }
}

fn walk(node: Node, src: &[u8], scan: &mut Scan) {
    let risk = &mut scan.risk;
    match node.kind() {
        "command" => {
            let nodes = arg_nodes(node);
            let words: Vec<String> = nodes.iter().map(|n| text(*n, src)).collect();
            bump(risk, command_risk(&words));
            scan.segments.opaque |= runs_code_string(&words);
            // From the name on: a deny rule matches past a leading
            // assignment, which already makes the call opaque to allow rules.
            let from = node.child_by_field_name("name").unwrap_or(node);
            let line = src
                .get(from.start_byte()..node.end_byte())
                .unwrap_or_default();
            let line = String::from_utf8_lossy(line).trim().to_owned();
            // A `MISSING` name after a dangling `&&` has no text; the parse
            // error has already made the call opaque.
            if !line.is_empty() {
                scan.segments.commands.push(line);
            }
            // T36.2: a deny/ask rule for the wrapped command must see it
            // past `nohup`, `timeout 5`, … too, not only the line as
            // written (research.md row 38).
            let inner = innermost(&nodes, src);
            if let (Some(first), Some(outer)) = (inner.first(), nodes.first())
                && first.start_byte() != outer.start_byte()
            {
                push_segment(scan, src, first.start_byte(), node.end_byte());
            }
            // T36.2: deny/ask must look inside `eval …`/`sh -c '…'` strings
            // the split otherwise cannot see into; allow never can (the
            // call stays opaque via `runs_code_string` above).
            if scan.depth < MAX_INNER_DEPTH
                && let Some(code) = inner_code(&nodes, src)
            {
                let (inner_risk, inner_segments) = scan_at(&code, scan.depth + 1);
                bump(&mut scan.risk, inner_risk);
                scan.segments.commands.extend(inner_segments.commands);
            }
        }
        "pipeline" if piped_into_interpreter(node, src) => bump(risk, Risk::Destructive),
        "file_redirect" => {
            let r = redirect_risk(node, src);
            bump(risk, r);
            scan.segments.opaque |= r != Risk::ReadOnly;
        }
        // Output fed back in as words the split never sees as commands.
        "command_substitution" | "process_substitution" => {
            bump(risk, Risk::Exec);
            scan.segments.opaque = true;
        }
        // Anything that forks a shell.
        "subshell" | "heredoc_redirect" | "herestring_redirect" => bump(risk, Risk::Exec),
        // A bare prefix (`FOO=x cmd`, or `FOO=x` alone): safe only when it
        // touches nothing but known-safe variables (T36.2).
        "variable_assignment" => {
            if !is_safe_assignment(node, src) {
                bump(risk, Risk::Exec);
                scan.segments.opaque = true;
            }
        }
        // `export`/`declare`/`typeset`/`readonly`/`local`, and `unset`: no
        // safe-list exception — these do more than set one value for the
        // rest of the line (T36.2).
        "declaration_command" | "unset_command" => {
            bump(risk, Risk::Exec);
            scan.segments.opaque = true;
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, src, scan);
    }
}

/// Pushes `src[from..to]`, trimmed, as one more command the permission
/// engine can match — shared by the top-level command line and the
/// wrapper-unwrapped text (T36.2).
fn push_segment(scan: &mut Scan, src: &[u8], from: usize, to: usize) {
    let seg = src.get(from..to).unwrap_or_default();
    let seg = String::from_utf8_lossy(seg).trim().to_owned();
    if !seg.is_empty() {
        scan.segments.commands.push(seg);
    }
}

/// `eval …` or `sh -c …`, possibly behind a wrapper: a string run as code.
fn runs_code_string(words: &[String]) -> bool {
    let Some((name, args)) = words.split_first() else {
        return false;
    };
    match base(name) {
        "eval" => true,
        n if SHELLS.contains(&n) => args
            .iter()
            .any(|a| a.starts_with('-') && !a.starts_with("--") && a.contains('c')),
        n => wrapper_args(n, args).is_some_and(runs_code_string),
    }
}

fn text(node: Node, src: &[u8]) -> String {
    node.utf8_text(src).unwrap_or_default().to_owned()
}

/// The command's own children in source order, dropping a leading `VAR=x`
/// assignment (it does not change what runs): index 0 is the name, the
/// rest are its arguments. Shared by the risk/string walk and the
/// node-based wrapper/`sh -c` unwrapping (T36.2), so both see the same
/// list.
fn arg_nodes<'a>(node: Node<'a>) -> Vec<Node<'a>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|c| c.kind() != "variable_assignment")
        .collect()
}

fn command_name(node: Node, src: &[u8]) -> Option<String> {
    node.child_by_field_name("name")
        .map(|n| base(&text(n, src)).to_owned())
}

fn base(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

/// Whether a leading `VAR=value` assignment (or a bare one with no
/// command after it) only sets a variable that cannot change what runs
/// (T36.2, `SAFE_ASSIGNMENTS`).
fn is_safe_assignment(node: Node, src: &[u8]) -> bool {
    node.child_by_field_name("name")
        .map(|n| text(n, src))
        .is_some_and(|name| SAFE_ASSIGNMENTS.contains(&name.as_str()))
}

/// A number-like duration (`5`, `5s`, `1.5m`) — `timeout`'s own positional
/// argument, which `wrapper_args` must skip along with any flags.
fn is_duration(s: &str) -> bool {
    let digits = s.strip_suffix(['s', 'm', 'h', 'd']).unwrap_or(s);
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// `args` beyond `name`'s own — its flags, plus `timeout`'s duration — when
/// `name` is one of `WRAPPERS`; `None` when it is not a wrapper at all,
/// including `xargs` carrying a flag of its own (research.md row 38: only
/// *bare* `xargs` is stripped, since one with flags is the command that
/// reads stdin and runs it). The single place risk classification and
/// segment extraction agree on what a wrapper hides (T36.2).
fn wrapper_args<'a, T: AsRef<str>>(name: &str, args: &'a [T]) -> Option<&'a [T]> {
    if !WRAPPERS.contains(&name) {
        return None;
    }
    if name == "xargs" && args.first().is_some_and(|a| a.as_ref().starts_with('-')) {
        return None;
    }
    let mut own = args
        .iter()
        .take_while(|a| a.as_ref().starts_with('-') || a.as_ref().contains('='))
        .count();
    if name == "timeout" && args.get(own).is_some_and(|a| is_duration(a.as_ref())) {
        own += 1;
    }
    Some(args.get(own..).unwrap_or_default())
}

fn command_risk(words: &[String]) -> Risk {
    let Some((name, args)) = words.split_first() else {
        return Risk::Exec;
    };
    let name = base(name);
    if let Some(inner) = wrapper_args(name, args) {
        return match (inner.is_empty(), name) {
            (true, "env") => Risk::ReadOnly,
            (true, _) => Risk::Exec,
            (false, _) => command_risk(inner),
        };
    }
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let short = |c: char| {
        args.iter()
            .any(|a| a.starts_with('-') && !a.starts_with("--") && a.contains(c))
    };
    match name {
        "sudo" | "doas" | "dd" | "shutdown" | "reboot" | "halt" => Risk::Destructive,
        n if n.starts_with("mkfs") => Risk::Destructive,
        "rm" if short('r') || short('R') || has("--recursive") => Risk::Destructive,
        "chmod" | "chown" | "chgrp" if short('R') || has("--recursive") => Risk::Destructive,
        "git" => git_risk(args),
        "cargo" => match args.first().map(String::as_str) {
            Some("check" | "test" | "build" | "clippy" | "metadata" | "tree" | "doc") => {
                Risk::ReadOnly
            }
            _ => Risk::Exec,
        },
        "npm" | "pnpm" | "yarn" if args == ["test"] || args == ["run", "test"] => Risk::ReadOnly,
        "find" if has("-delete") || has("-exec") || has("-execdir") || has("-ok") => Risk::Exec,
        "find" => Risk::ReadOnly,
        n if READ_ONLY.contains(&n) => Risk::ReadOnly,
        _ => Risk::Exec,
    }
}

/// The nodes for the command a wrapper prefix (`nohup`, `timeout 5`, `env
/// FOO=1`, …) actually runs, peeled as many layers deep as apply — the
/// same rule `wrapper_args` uses for risk, so `nohup timeout 5 rm x`
/// unwraps fully to `rm x` (T36.2). Returns `nodes` unchanged when `name`
/// is not a stripped wrapper.
fn innermost<'a>(nodes: &'a [Node<'a>], src: &[u8]) -> &'a [Node<'a>] {
    let Some((name_node, args)) = nodes.split_first() else {
        return nodes;
    };
    let name_text = text(*name_node, src);
    let name = base(&name_text);
    let texts: Vec<String> = args.iter().map(|a| text(*a, src)).collect();
    match wrapper_args(name, &texts) {
        Some(inner) if !inner.is_empty() => innermost(&args[texts.len() - inner.len()..], src),
        _ => nodes,
    }
}

/// The literal script an `eval …`/`sh -c '…'`/`bash -c "…"` node runs, if
/// one is visible in the parse — peeled through a wrapper first (`env sh
/// -c '…'`, `timeout 5 bash -c '…'`) so deny/ask can walk it too (T36.2).
/// `None` when there is nothing to recover: `eval` with no arguments, or a
/// shell's `-c` flag with nothing after it.
fn inner_code(nodes: &[Node], src: &[u8]) -> Option<String> {
    let effective = innermost(nodes, src);
    let (name_node, args) = effective.split_first()?;
    let name_text = text(*name_node, src);
    let name = base(&name_text);
    if name == "eval" {
        if args.is_empty() {
            return None;
        }
        return Some(
            args.iter()
                .map(|a| unquote(&text(*a, src)))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    if SHELLS.contains(&name) {
        let flag = args.iter().position(|a| {
            let t = text(*a, src);
            t.starts_with('-') && !t.starts_with("--") && t.contains('c')
        })?;
        let script = args.get(flag + 1)?;
        return Some(unquote(&text(*script, src)));
    }
    None
}

/// Strips one layer of matching quotes bash would remove before running
/// the string — enough for the plain single- or double-quoted scripts
/// `sh -c` normally carries; a script that starts and ends with the same
/// quote character it also contains inside is left as written, which only
/// widens what the re-parse sees, never narrows it.
fn unquote(s: &str) -> String {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let (a, b) = (bytes[0], bytes[bytes.len() - 1]);
        if (a == b'\'' && b == b'\'') || (a == b'"' && b == b'"') {
            return s[1..s.len() - 1].to_owned();
        }
    }
    s.to_owned()
}

fn git_risk(args: &[String]) -> Risk {
    let mut it = args.iter();
    let mut sub = None;
    while let Some(a) = it.next() {
        if a == "-C" || a == "-c" {
            it.next();
        } else if !a.starts_with('-') {
            sub = Some(a.as_str());
            break;
        }
    }
    let has = |flag: &str| args.iter().any(|a| a == flag);
    match sub {
        Some("status" | "diff" | "log" | "show" | "blame" | "rev-parse" | "ls-files") => {
            Risk::ReadOnly
        }
        Some("push") if has("--force") || has("-f") || has("--force-with-lease") => {
            Risk::Destructive
        }
        Some("reset") if has("--hard") => Risk::Destructive,
        Some("clean") => Risk::Destructive,
        _ => Risk::Exec,
    }
}

/// `curl … | sh` and friends: remote code straight into an interpreter.
fn piped_into_interpreter(node: Node, src: &[u8]) -> bool {
    let mut cursor = node.walk();
    let names: Vec<String> = node
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "command")
        .filter_map(|c| command_name(c, src))
        .collect();
    names
        .iter()
        .position(|n| DOWNLOADERS.contains(&n.as_str()))
        .is_some_and(|i| {
            names[i + 1..]
                .iter()
                .any(|n| INTERPRETERS.contains(&n.as_str()))
        })
}

fn redirect_risk(node: Node, src: &[u8]) -> Risk {
    let dest = node
        .child_by_field_name("destination")
        .map(|d| text(d, src))
        .unwrap_or_default();
    let mut cursor = node.walk();
    let op = node
        .children(&mut cursor)
        .find(|c| !c.is_named())
        .map(|c| text(c, src))
        .unwrap_or_default();
    // Reading stdin or duplicating a descriptor changes no file.
    if (op.contains('<') && !op.contains('>')) || dest.chars().all(|c| c.is_ascii_digit()) {
        return Risk::ReadOnly;
    }
    if dest.starts_with("/dev/") {
        return if HARMLESS_DEVICES.contains(&dest.as_str()) {
            Risk::ReadOnly
        } else {
            Risk::Destructive
        };
    }
    Risk::Exec
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T57.2: on Windows without Git Bash the line is PowerShell (or `cmd`),
    /// which the bash grammar cannot parse. Such a line is `Exec` with
    /// opaque segments, so no allow rule matches it and the engine asks.
    #[test]
    fn unparsed_powershell_command_asks() {
        for line in [
            "if (Test-Path .\\build) { Remove-Item .\\build -Recurse -Force }",
            "$items = @(Get-ChildItem -Recurse); $items | Remove-Item -Force",
            "for /f \"tokens=*\" %i in ('dir /b') do @del %i",
        ] {
            assert_eq!(classify(line), Risk::Exec, "{line}");
            assert!(segments(line).opaque, "{line}");
        }
    }
}
