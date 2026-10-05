// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The named-placeholder convention of the catalogs. gettext has no named
//! arguments, so cox writes them the way Python's `str.format` does, which
//! GNU gettext knows as `python-brace-format` (and `msgfmt --check`
//! validates): `{name}` is a placeholder whose name is an ASCII identifier,
//! `{{` and `}}` are literal braces. Nothing else inside braces (`{0}`,
//! `{name:>4}`) is a placeholder; [`validate`] reports it so a catalog cannot
//! ship one by accident.

/// A piece of a parsed template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    Var(String),
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Splits `template` into text and `{name}` placeholders. Malformed braces
/// stay as text (see [`validate`]).
pub fn pieces(template: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        text.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            text.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        if let Some(end) = tail.strip_prefix('{').and_then(|t| t.find('}'))
            && is_ident(&tail[1..=end])
        {
            if !text.is_empty() {
                out.push(Piece::Text(std::mem::take(&mut text)));
            }
            out.push(Piece::Var(tail[1..=end].to_owned()));
            rest = &tail[end + 2..];
            continue;
        }
        text.push_str(&tail[..1]);
        rest = &tail[1..];
    }
    text.push_str(rest);
    if !text.is_empty() {
        out.push(Piece::Text(text));
    }
    out
}

/// The distinct placeholder names in `template`, in order of first use.
pub fn placeholders(template: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in pieces(template) {
        if let Piece::Var(v) = p
            && !out.contains(&v)
        {
            out.push(v);
        }
    }
    out
}

/// `template` with each placeholder replaced by `value(name)`; a placeholder
/// with no value stays spelled `{name}`, so a missing argument is visible.
pub fn render(template: &str, value: impl Fn(&str) -> Option<String>) -> String {
    let mut s = String::with_capacity(template.len());
    for p in pieces(template) {
        match p {
            Piece::Text(t) => s.push_str(&t),
            Piece::Var(v) => match value(&v) {
                Some(x) => s.push_str(&x),
                None => {
                    s.push('{');
                    s.push_str(&v);
                    s.push('}');
                }
            },
        }
    }
    s
}

/// Why `template` breaks the convention, if it does: a brace that is neither
/// doubled nor part of a `{name}` placeholder.
pub fn validate(template: &str) -> Result<(), String> {
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            rest = &tail[2..];
            continue;
        }
        match tail.strip_prefix('{').and_then(|t| t.find('}')) {
            Some(end) if is_ident(&tail[1..=end]) => rest = &tail[end + 2..],
            _ => {
                let snippet: String = tail.chars().take(12).collect();
                return Err(format!(
                    "`{snippet}`: placeholders are `{{name}}`, literal braces are doubled"
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_escapes_and_missing_values() {
        let t = "{{x}} {name} has {count} of {count}}}";
        assert_eq!(placeholders(t), ["name", "count"]);
        let out = render(t, |v| (v == "count").then(|| "3".to_owned()));
        assert_eq!(out, "{x} {name} has 3 of 3}");
        assert!(validate(t).is_ok());
        for bad in ["{0}", "{name:>4}", "a { b", "}"] {
            assert!(validate(bad).is_err(), "`{bad}` validated");
        }
    }
}
