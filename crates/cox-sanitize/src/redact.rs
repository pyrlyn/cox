// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Unconditional redaction of what leaves a session (T28.4): one pattern
//! table and one `scrub` helper behind every output boundary — rollout
//! lines, logs, headless output, exports, a plugin's context snapshot. Moved
//! here from `cox-core::redact` by T33.9 because the plugin host
//! (`cox-plugin`, which may not depend on `cox-core`) folds its own snapshot
//! and must redact it with the same patterns; `cox-core` re-exports it at the
//! old `cox_core::redact::{scrub, REDACTED}` path. Pure `&str` → `&str`, like
//! `sanitize`, so this crate still depends on no workspace crate.

use std::borrow::Cow;

/// What a secret-shaped run becomes. The same marker `cox record --redact`
/// (T1.5) leaves in cassettes, so redacted bytes look identical everywhere.
pub const REDACTED: &str = "«redacted»";

/// Replaces secret-shaped runs in `text` — `sk-…` keys, `Bearer …` tokens,
/// AWS `AKIA…` key ids, GitHub `ghp_…` tokens and PEM blocks — and returns
/// the original borrow when nothing matched, so clean output is copied by
/// neither this function nor its callers.
pub fn scrub(text: &str) -> Cow<'_, str> {
    // Every shape starts with one of these; without one the scanner below
    // cannot change anything.
    if !["sk-", "Bearer ", "AKIA", "ghp_", "-----BEGIN "]
        .iter()
        .any(|marker| text.contains(marker))
    {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut changed = false;
    while !rest.is_empty() {
        match secret_span(rest) {
            Some(len) => {
                out.push_str(REDACTED);
                rest = &rest[len..];
                changed = true;
            }
            None => {
                let Some(ch) = rest.chars().next() else {
                    break;
                };
                out.push(ch);
                rest = &rest[ch.len_utf8()..];
            }
        }
    }
    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(text)
    }
}

/// The byte length of the secret-shaped run at the start of `s`, if any.
fn secret_span(s: &str) -> Option<usize> {
    if let Some(after) = s.strip_prefix("Bearer ") {
        // Any run to the next whitespace is a bearer token.
        let end = after.find(char::is_whitespace).unwrap_or(after.len());
        return Some("Bearer ".len() + end);
    }
    if let Some(after) = s.strip_prefix("sk-") {
        // Anthropic (`sk-ant-api03-…`) and OpenAI project keys carry `-` and
        // `_` in the body; an alphanumeric-only body stopped at `ant`.
        return prefixed(3, after, 8, |b| {
            b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')
        });
    }
    if let Some(after) = s.strip_prefix("AKIA") {
        return prefixed(4, after, 16, u8::is_ascii_alphanumeric);
    }
    if let Some(after) = s.strip_prefix("ghp_") {
        return prefixed(4, after, 8, u8::is_ascii_alphanumeric);
    }
    s.starts_with("-----BEGIN ").then(|| {
        // A PEM block runs to the end of its `-----END …-----` line; an
        // unterminated one means the key material was pasted without a tail.
        s.find("\n-----END ").map_or(s.len(), |at| {
            s[at + 1..].find('\n').map_or(s.len(), |nl| at + 1 + nl)
        })
    })
}

/// `prefix` bytes plus at least `min` body bytes, or no match.
fn prefixed(prefix: usize, after: &str, min: usize, body: fn(&u8) -> bool) -> Option<usize> {
    let n = after.bytes().take_while(body).count();
    (n >= min).then_some(prefix + n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_table() {
        // The block's trailing newline stays, like every other pattern's tail.
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIEow\n-----END RSA PRIVATE KEY-----";
        let cases: &[(&str, &str)] = &[
            ("sk-abc12345678", REDACTED),
            ("Bearer tokensecret", REDACTED),
            ("AKIA0123456789ABCDEF", REDACTED),
            ("ghp_0123456789abcdef0123", REDACTED),
            (pem, REDACTED),
            // T1.5 parity: both shapes in one line, non-ASCII preserved.
            (
                "key=sk-abcdefghijk Authorization: Bearer tokensecret",
                "key=«redacted» Authorization: «redacted»",
            ),
            ("café sk-abcdefghijk 日本語", "café «redacted» 日本語"),
            // Below the length floors, and ordinary text, stay verbatim.
            ("sk-shrt", "sk-shrt"),
            ("AKIA0123", "AKIA0123"),
            ("plain text 1234", "plain text 1234"),
        ];
        for (input, want) in cases {
            assert_eq!(scrub(input).as_ref(), *want, "{input:?}");
        }
        assert!(matches!(scrub("plain text 1234"), Cow::Borrowed(_)));
    }

    #[test]
    fn scrub_redacts_an_anthropic_key_whole() {
        let line = "x-api-key: sk-ant-api03-R2D2_c3po-XyZ0123456789-abcDEF_ghiAA end";
        assert_eq!(scrub(line), "x-api-key: «redacted» end");
    }
}
