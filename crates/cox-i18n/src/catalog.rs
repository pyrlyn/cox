// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! One `.po` catalog parsed with `polib` into what lookups need: entries by
//! key, in file order, and the compiled `Plural-Forms` rule.

use std::collections::HashMap;

use crate::I18nError;
use crate::plural::PluralRule;

/// Translator-comment prefix that gives a plural message its CLDR `other`
/// form (fractions) when the language's gettext forms have none of their own
/// (`docs/i18n.md`, "Plurals").
pub const OTHER_COMMENT: &str = "cldr-other:";

/// One message of a catalog.
#[derive(Debug, Clone)]
pub struct Entry {
    /// `msgctxt` when set, otherwise `msgid`: what `t()` looks up.
    pub key: String,
    /// The English source text.
    pub msgid: String,
    /// The English plural source, for plural messages.
    pub msgid_plural: Option<String>,
    /// `msgstr`, or `msgstr[0..]` for a plural message; empty when untranslated.
    pub forms: Vec<String>,
    /// Marked `#, fuzzy`: gettext never shows such a translation.
    pub fuzzy: bool,
    /// The extracted comments (`#.`), joined with spaces.
    pub comment: String,
    /// The `# cldr-other:` override, when the translator wrote one.
    pub other: Option<String>,
}

impl Entry {
    /// The translated forms, or `None` when the entry must fall back: fuzzy,
    /// an empty `msgstr`, or a plural message missing any of its
    /// `nplurals` forms.
    pub fn translated(&self, nplurals: usize) -> Option<&[String]> {
        let want = if self.msgid_plural.is_some() {
            nplurals
        } else {
            1
        };
        let ok =
            !self.fuzzy && self.forms.len() == want && self.forms.iter().all(|f| !f.is_empty());
        ok.then_some(self.forms.as_slice())
    }
}

/// A parsed catalog.
#[derive(Debug, Clone)]
pub struct Catalog {
    pub rule: PluralRule,
    /// The raw `Plural-Forms` value, normalised by polib (`nplurals=…; plural=…;`).
    pub plural_forms: String,
    entries: Vec<Entry>,
    by_key: HashMap<String, usize>,
}

impl Catalog {
    /// Parses `source`, the text of `<code>.po`.
    pub fn parse(code: &str, source: &str) -> Result<Self, I18nError> {
        // polib panics on a bare `#` before any field has been read.
        if source
            .lines()
            .find(|l| !l.trim().is_empty())
            .map(str::trim_end)
            == Some("#")
        {
            return Err(I18nError::Parse {
                code: code.to_owned(),
                detail: "the file starts with a bare `#` line".to_owned(),
            });
        }
        let po =
            polib::po_file::parse_from_reader(source.as_bytes()).map_err(|e| I18nError::Parse {
                code: code.to_owned(),
                detail: e.to_string(),
            })?;
        let rules = &po.metadata.plural_rules;
        let rule =
            PluralRule::new(rules.nplurals, &rules.expr).map_err(|e| I18nError::PluralForms {
                code: code.to_owned(),
                detail: e.0,
            })?;
        let mut entries = Vec::new();
        let mut by_key = HashMap::new();
        for m in po.messages() {
            let key = m.msgctxt().unwrap_or(m.msgid()).to_owned();
            let forms = match (m.msgstr(), m.msgstr_plural()) {
                (Ok(s), _) => vec![s.to_owned()],
                (_, Ok(p)) => p.clone(),
                _ => Vec::new(),
            };
            let other = m.translator_comments().lines().find_map(|l| {
                l.trim()
                    .strip_prefix(OTHER_COMMENT)
                    .map(|s| s.trim().to_owned())
            });
            let entry = Entry {
                key: key.clone(),
                msgid: m.msgid().to_owned(),
                msgid_plural: m.msgid_plural().ok().map(str::to_owned),
                forms,
                fuzzy: m.is_fuzzy(),
                comment: m
                    .extracted_comments()
                    .lines()
                    .map(str::trim)
                    .collect::<Vec<_>>()
                    .join(" "),
                other,
            };
            if by_key.insert(key.clone(), entries.len()).is_some() {
                return Err(I18nError::Duplicate {
                    code: code.to_owned(),
                    key,
                });
            }
            entries.push(entry);
        }
        Ok(Self {
            rule,
            plural_forms: rules.dump(),
            entries,
            by_key,
        })
    }

    pub fn get(&self, key: &str) -> Option<&Entry> {
        self.by_key.get(key).map(|&i| &self.entries[i])
    }

    /// Every entry, in file order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_files_are_errors_not_panics() {
        let hdr = "msgid \"\"\nmsgstr \"Plural-Forms: nplurals=2; plural=(n != 1);\\n\"\n";
        assert!(Catalog::parse("xx", &format!("#\n{hdr}")).is_err());
        assert!(
            Catalog::parse("xx", "msgid \"a\"\nmsgstr \"b\"\n").is_err(),
            "no header"
        );
        let bad_rule = "msgid \"\"\nmsgstr \"Plural-Forms: nplurals=2; plural=(n !! 1);\\n\"\n";
        assert!(matches!(
            Catalog::parse("xx", bad_rule),
            Err(I18nError::PluralForms { .. })
        ));
        let dup = format!(
            "{hdr}\nmsgctxt \"k\"\nmsgid \"a\"\nmsgstr \"\"\n\nmsgctxt \"k\"\nmsgid \"b\"\nmsgstr \"\"\n"
        );
        assert!(matches!(
            Catalog::parse("xx", &dup),
            Err(I18nError::Duplicate { .. })
        ));
        assert!(Catalog::parse("xx", hdr).is_ok());
    }
}
