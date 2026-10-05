// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Lowers the embedded gettext catalogs to the native apps' resource formats:
//! Apple `<code>.lproj/Localizable.strings` (plain messages) and
//! `Localizable.stringsdict` (plural messages) for `desktop/macos`, and
//! Windows `Strings/<tag>/Resources.resw` for the planned WinUI app
//! (`desktop/windows`).
//!
//! Messages a translation lacks (missing, empty or fuzzy) are filled from
//! `en` (its `msgstr`, else the `msgid`), so the native apps see the same
//! per-message fallback as the Rust side. gettext plural forms are
//! positional (`msgstr[0..nplurals]`); [`Locale::plural_categories`] names
//! the CLDR category of each, and CLDR `other` (fractions, which gettext
//! cannot count) comes from the translator's `# cldr-other:` comment or the
//! [`Locale::other_form`] form (`docs/i18n.md`).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::catalog::{Catalog, Entry};
use crate::format::{self, Piece};
use crate::{BRAND_NAME, DEFAULT_LOCALE, I18nError, LOCALES, Locale};

/// The variable that selects a plural form, and so the one `%lld` argument.
const COUNT: &str = "count";

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error(transparent)]
    Catalog(#[from] I18nError),
    #[error("locale `{code}`, message `{id}`: {reason}")]
    Unsupported {
        code: String,
        id: String,
        reason: String,
    },
    #[error("writing {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, PartialEq)]
enum Shape {
    Simple(Vec<Piece>),
    /// One complete text per CLDR category, in form order, `other` last.
    Plural(Vec<(&'static str, Vec<Piece>)>),
}

#[derive(Debug, Clone)]
struct Message {
    id: String,
    shape: Shape,
    comment: String,
    /// Filled from `en` because the locale lacks a usable translation.
    fallback: bool,
}

/// One locale ready to render, messages in `en` catalog order.
struct Lowered {
    locale: &'static Locale,
    messages: Vec<Message>,
}

/// Writes every locale under `out`: `apple/<code>.lproj/…` and
/// `windows/Strings/<tag>/Resources.resw`. Returns the files written.
pub fn export_all(out: &Path) -> Result<Vec<PathBuf>, ExportError> {
    let lowered = lower_all(LOCALES)?;
    let order = lowered
        .iter()
        .find(|l| l.locale.code == DEFAULT_LOCALE)
        .map(|l| arg_order(&l.messages))
        .unwrap_or_default();
    let mut written = Vec::new();
    for l in &lowered {
        let lproj = out.join("apple").join(format!("{}.lproj", l.locale.code));
        let strings = render_strings(&l.messages, &order);
        written.push(write(&lproj.join("Localizable.strings"), &strings)?);
        let dict = render_stringsdict(&l.messages, &order);
        written.push(write(&lproj.join("Localizable.stringsdict"), &dict)?);
        let resw_dir = out.join("windows").join("Strings").join(l.locale.windows);
        let resw = render_resw(&l.messages, &order);
        written.push(write(&resw_dir.join("Resources.resw"), &resw)?);
    }
    Ok(written)
}

fn write(path: &Path, contents: &str) -> Result<PathBuf, ExportError> {
    let io = |source| ExportError::Io {
        path: path.to_owned(),
        source,
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    std::fs::write(path, contents).map_err(io)?;
    Ok(path.to_owned())
}

fn unsupported(code: &str, id: &str, reason: impl Into<String>) -> ExportError {
    ExportError::Unsupported {
        code: code.to_owned(),
        id: id.to_owned(),
        reason: reason.into(),
    }
}

/// Parses a template with `{brand}` inlined as text: the product name is
/// fixed in every format.
fn lower_text(code: &str, id: &str, text: &str) -> Result<Vec<Piece>, ExportError> {
    format::validate(text).map_err(|e| unsupported(code, id, e))?;
    let mut out: Vec<Piece> = Vec::new();
    for p in format::pieces(text) {
        let p = match p {
            Piece::Var(v) if v == "brand" => Piece::Text(BRAND_NAME.to_owned()),
            p => p,
        };
        match (out.last_mut(), p) {
            (Some(Piece::Text(t)), Piece::Text(more)) => t.push_str(&more),
            (_, p) => out.push(p),
        }
    }
    Ok(out)
}

/// The shape of `entry`'s own translation in `locale`, or `None` when it has
/// none and must be filled from `en`.
fn own_shape(
    locale: &Locale,
    catalog: &Catalog,
    entry: &Entry,
) -> Result<Option<Shape>, ExportError> {
    let (code, id) = (locale.code, entry.key.as_str());
    let Some(forms) = entry.translated(catalog.rule.nplurals()) else {
        if !entry.fuzzy && entry.forms.iter().any(|f| !f.is_empty()) {
            let reason = format!(
                "{} plural form(s) where Plural-Forms says nplurals={}",
                entry.forms.len(),
                catalog.rule.nplurals()
            );
            return Err(unsupported(code, id, reason));
        }
        return Ok(None);
    };
    if entry.msgid_plural.is_none() {
        return Ok(Some(Shape::Simple(lower_text(code, id, &forms[0])?)));
    }
    let mut variants = Vec::new();
    for (category, text) in locale.plural_categories.iter().zip(forms) {
        variants.push((*category, lower_text(code, id, text)?));
    }
    if !locale.plural_categories.contains(&"other") {
        let text = entry
            .other
            .as_ref()
            .or_else(|| forms.get(locale.other_form))
            .ok_or_else(|| unsupported(code, id, "other_form is not a form index"))?;
        variants.push(("other", lower_text(code, id, text)?));
    }
    Ok(Some(Shape::Plural(variants)))
}

/// `en`'s shape for `entry`: its translation, else the `msgid`/`msgid_plural`.
fn en_shape(en: &Locale, catalog: &Catalog, entry: &Entry) -> Result<Shape, ExportError> {
    if let Some(shape) = own_shape(en, catalog, entry)? {
        return Ok(shape);
    }
    let id = entry.key.as_str();
    Ok(match &entry.msgid_plural {
        None => Shape::Simple(lower_text(en.code, id, &entry.msgid)?),
        Some(plural) => Shape::Plural(vec![
            ("one", lower_text(en.code, id, &entry.msgid)?),
            ("other", lower_text(en.code, id, plural)?),
        ]),
    })
}

/// Lowers every locale and fills each translation's gaps from `en`.
fn lower_all(locales: &'static [Locale]) -> Result<Vec<Lowered>, ExportError> {
    let en_locale = locales
        .iter()
        .find(|l| l.code == DEFAULT_LOCALE)
        .ok_or_else(|| unsupported(DEFAULT_LOCALE, "", "no en locale"))?;
    let en = Catalog::parse(en_locale.code, en_locale.source)?;
    let mut en_messages = Vec::new();
    for entry in en.entries() {
        if entry
            .msgid_plural
            .as_ref()
            .is_some_and(|p| !p.contains("{count}"))
        {
            return Err(unsupported(
                "en",
                &entry.key,
                "a plural message must use {count}",
            ));
        }
        en_messages.push(Message {
            id: entry.key.clone(),
            shape: en_shape(en_locale, &en, entry)?,
            comment: entry.comment.clone(),
            fallback: false,
        });
    }
    let en_vars = arg_order(&en_messages);
    let mut out = Vec::new();
    for locale in locales {
        let catalog = Catalog::parse(locale.code, locale.source)?;
        if catalog.rule.nplurals() != locale.plural_categories.len() {
            let reason = format!(
                "Plural-Forms has nplurals={}, LOCALES names {} categories",
                catalog.rule.nplurals(),
                locale.plural_categories.len()
            );
            return Err(unsupported(locale.code, "", reason));
        }
        if let Some(extra) = catalog.entries().iter().find(|e| en.get(&e.key).is_none()) {
            return Err(unsupported(
                locale.code,
                &extra.key,
                "the id is not defined in en",
            ));
        }
        let mut messages = Vec::new();
        for source in &en_messages {
            let own = match catalog.get(&source.id) {
                Some(entry) => own_shape(locale, &catalog, entry)?,
                None => None,
            };
            let message = match own {
                Some(shape) => Message {
                    shape,
                    fallback: false,
                    ..source.clone()
                },
                None => Message {
                    fallback: locale.code != DEFAULT_LOCALE,
                    ..source.clone()
                },
            };
            let allowed = en_vars.get(&message.id).cloned().unwrap_or_default();
            if let Some(v) = vars(&message.shape)
                .into_iter()
                .find(|v| !allowed.contains(v))
            {
                let reason = format!("`{{{v}}}` is not a placeholder of the en message");
                return Err(unsupported(locale.code, &message.id, reason));
            }
            messages.push(message);
        }
        out.push(Lowered { locale, messages });
    }
    Ok(out)
}

/// Positional argument order per message id: placeholders in order of first
/// appearance in the `en` message. Translations reuse it, so `{0}`/`%1$@`
/// mean the same argument in every locale even when a translation reorders.
fn arg_order(en: &[Message]) -> HashMap<String, Vec<String>> {
    en.iter().map(|m| (m.id.clone(), vars(&m.shape))).collect()
}

fn vars(shape: &Shape) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut push = |pieces: &[Piece]| {
        for p in pieces {
            if let Piece::Var(v) = p
                && !seen.contains(v)
            {
                seen.push(v.clone());
            }
        }
    };
    match shape {
        Shape::Simple(p) => push(p),
        Shape::Plural(variants) => {
            for (_, v) in variants {
                push(v);
            }
        }
    }
    seen
}

fn comment_text(m: &Message) -> String {
    let mut parts = Vec::new();
    if !m.comment.is_empty() {
        parts.push(m.comment.clone());
    }
    if m.fallback {
        parts.push(format!("fallback: {DEFAULT_LOCALE}"));
    }
    parts.join(" | ")
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ---- Apple ------------------------------------------------------------------

/// Apple format text: `%` doubled, placeholders as `%@`, and `{count}` in a
/// plural message as `%lld`; positional (`%2$@`) once a message has more
/// than one argument.
fn apple(pieces: &[Piece], order: &[String], plural: bool) -> String {
    let mut s = String::new();
    for p in pieces {
        match p {
            Piece::Text(t) => s.push_str(&t.replace('%', "%%")),
            Piece::Var(v) => {
                let kind = if plural && v == COUNT { "lld" } else { "@" };
                s.push_str(&apple_spec(v, order, kind));
            }
        }
    }
    s
}

fn apple_spec(var: &str, order: &[String], kind: &str) -> String {
    match order.iter().position(|v| v == var) {
        Some(i) if order.len() > 1 => format!("%{}${kind}", i + 1),
        _ => format!("%{kind}"),
    }
}

fn strings_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

const GENERATED: &str = "Generated by po-export (cox-i18n) from the gettext catalogs. Do not edit.";

fn render_strings(messages: &[Message], order: &HashMap<String, Vec<String>>) -> String {
    let mut s = format!("/* {GENERATED} */\n");
    for m in messages {
        let Shape::Simple(pieces) = &m.shape else {
            continue;
        };
        let args = order.get(&m.id).map(Vec::as_slice).unwrap_or_default();
        let note = comment_text(m);
        if !note.is_empty() {
            let _ = write!(s, "\n/* {} */", note.replace("*/", "* /"));
        }
        let _ = write!(
            s,
            "\n\"{}\" = \"{}\";\n",
            strings_escape(&m.id),
            strings_escape(&apple(pieces, args, false))
        );
    }
    s
}

fn render_stringsdict(messages: &[Message], order: &HashMap<String, Vec<String>>) -> String {
    let mut s = String::from(concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" ",
        "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
    ));
    let _ = writeln!(s, "<!-- {GENERATED} -->\n<plist version=\"1.0\">\n<dict>");
    for m in messages {
        let Shape::Plural(variants) = &m.shape else {
            continue;
        };
        let args = order.get(&m.id).map(Vec::as_slice).unwrap_or_default();
        // Each variant is the whole text (gettext forms are complete
        // strings), so the format key is just the plural placeholder.
        let format_key = apple_spec(COUNT, args, &format!("#@{COUNT}@"));
        let note = comment_text(m);
        if !note.is_empty() {
            let _ = writeln!(s, "  <!-- {} -->", xml_escape(&note).replace("--", "- -"));
        }
        let _ = writeln!(s, "  <key>{}</key>\n  <dict>", xml_escape(&m.id));
        let _ = writeln!(
            s,
            "    <key>NSStringLocalizedFormatKey</key>\n    <string>{}</string>",
            xml_escape(&format_key)
        );
        let _ = writeln!(s, "    <key>{COUNT}</key>\n    <dict>");
        s.push_str("      <key>NSStringFormatSpecTypeKey</key>\n");
        s.push_str("      <string>NSStringPluralRuleType</string>\n");
        s.push_str("      <key>NSStringFormatValueTypeKey</key>\n      <string>lld</string>\n");
        for (category, pieces) in variants {
            let _ = writeln!(
                s,
                "      <key>{category}</key>\n      <string>{}</string>",
                xml_escape(&apple(pieces, args, true))
            );
        }
        s.push_str("    </dict>\n  </dict>\n");
    }
    s.push_str("</dict>\n</plist>\n");
    s
}

// ---- Windows ----------------------------------------------------------------

/// .NET composite format text: `{`/`}` doubled, placeholders as `{0}`,
/// `{1}` in the `en` argument order.
fn dotnet(pieces: &[Piece], order: &[String]) -> String {
    let mut s = String::new();
    for p in pieces {
        match p {
            Piece::Text(t) => s.push_str(&t.replace('{', "{{").replace('}', "}}")),
            Piece::Var(v) => {
                let i = order.iter().position(|o| o == v).unwrap_or_default();
                let _ = write!(s, "{{{i}}}");
            }
        }
    }
    s
}

const RESW_HEADER: &str = concat!(
    "<root>\n",
    "  <resheader name=\"resmimetype\">\n    <value>text/microsoft-resx</value>\n  </resheader>\n",
    "  <resheader name=\"version\">\n    <value>2.0</value>\n  </resheader>\n",
    "  <resheader name=\"reader\">\n",
    "    <value>System.Resources.ResXResourceReader, System.Windows.Forms, ",
    "Version=4.0.0.0, Culture=neutral, PublicKeyToken=b77a5c561934e089</value>\n",
    "  </resheader>\n",
    "  <resheader name=\"writer\">\n",
    "    <value>System.Resources.ResXResourceWriter, System.Windows.Forms, ",
    "Version=4.0.0.0, Culture=neutral, PublicKeyToken=b77a5c561934e089</value>\n",
    "  </resheader>\n",
);

/// `.resw` has no plural rules: a plural message becomes one entry per CLDR
/// category, `<id>_<category>`, and the app picks the category
/// (`docs/i18n.md`).
fn render_resw(messages: &[Message], order: &HashMap<String, Vec<String>>) -> String {
    let mut s = format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!-- {GENERATED} -->\n");
    s.push_str(RESW_HEADER);
    let mut entry = |name: &str, value: &str, note: &str| {
        let _ = writeln!(
            s,
            "  <data name=\"{}\" xml:space=\"preserve\">\n    <value>{}</value>",
            xml_escape(name),
            xml_escape(value)
        );
        if !note.is_empty() {
            let _ = writeln!(s, "    <comment>{}</comment>", xml_escape(note));
        }
        s.push_str("  </data>\n");
    };
    for m in messages {
        let args = order.get(&m.id).map(Vec::as_slice).unwrap_or_default();
        let note = comment_text(m);
        match &m.shape {
            Shape::Simple(pieces) => entry(&m.id, &dotnet(pieces, args), &note),
            Shape::Plural(variants) => {
                for (category, pieces) in variants {
                    entry(
                        &format!("{}_{category}", m.id),
                        &dotnet(pieces, args),
                        &note,
                    );
                }
            }
        }
    }
    s.push_str("</root>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lowered(code: &str) -> Lowered {
        lower_all(LOCALES)
            .expect("lowers")
            .into_iter()
            .find(|l| l.locale.code == code)
            .expect("locale exists")
    }

    fn en_order() -> HashMap<String, Vec<String>> {
        arg_order(&lowered("en").messages)
    }

    #[test]
    fn translations_are_filled_from_english_per_message() {
        for code in ["ru", "uk"] {
            let l = lowered(code);
            let m = l
                .messages
                .iter()
                .find(|m| m.id == "send-feedback")
                .expect("filled");
            assert!(m.fallback);
            assert_eq!(
                m.shape,
                Shape::Simple(vec![Piece::Text("Send feedback".into())])
            );
            let own = l
                .messages
                .iter()
                .find(|m| m.id == "settings-title")
                .expect("own");
            assert!(!own.fallback);
        }
        assert!(lowered("en").messages.iter().all(|m| !m.fallback));
    }

    #[test]
    fn apple_strings_inline_the_brand_and_map_variables() {
        let l = lowered("uk");
        let s = render_strings(&l.messages, &en_order());
        assert!(s.contains("\"quit-app\" = \"Вийти з Cox\";"), "{s}");
        assert!(
            s.contains("\"welcome-user\" = \"Ласкаво просимо до Cox, %@!\";"),
            "{s}"
        );
        assert!(
            s.contains("fallback: en */\n\"send-feedback\" = \"Send feedback\";"),
            "{s}"
        );
        assert!(
            !s.contains("session-count"),
            "plurals belong in the stringsdict"
        );
    }

    #[test]
    fn stringsdict_uses_positional_arguments_for_two_variables() {
        let l = lowered("ru");
        let s = render_stringsdict(&l.messages, &en_order());
        assert!(s.contains("<string>%2$#@count@</string>"), "{s}");
        assert!(
            s.contains("<key>many</key>\n      <string>%1$@ изменяет %2$lld файлов</string>"),
            "{s}"
        );
        assert!(s.contains("<string>%#@count@</string>"), "{s}");
        assert!(
            s.contains("<key>few</key>\n      <string>%lld сессии</string>"),
            "{s}"
        );
        // `other` (fractions) repeats the few form unless overridden.
        assert!(
            s.contains("<key>other</key>\n      <string>%lld сессии</string>"),
            "{s}"
        );
    }

    #[test]
    fn resw_splits_plurals_into_category_keys() {
        let l = lowered("uk");
        let s = render_resw(&l.messages, &en_order());
        for category in ["one", "few", "many", "other"] {
            assert!(
                s.contains(&format!("name=\"session-count_{category}\"")),
                "{s}"
            );
        }
        assert!(s.contains("<value>{0} змінює {1} файлів</value>"), "{s}");
        // The `# cldr-other:` override in uk.po.
        assert!(s.contains("name=\"files-changed_other\" xml:space=\"preserve\">\n    <value>{0} змінює {1} файлу</value>"), "{s}");
        assert!(
            s.contains("<value>Ласкаво просимо до Cox, {0}!</value>"),
            "{s}"
        );
        let en = render_resw(&lowered("en").messages, &en_order());
        assert!(en.contains("name=\"session-count_one\""), "{en}");
        assert!(!en.contains("session-count_few"), "{en}");
    }

    #[test]
    fn xml_comments_hold_no_double_hyphen() {
        // `--` inside `<!-- -->` makes the file malformed XML (xmllint rejects it).
        let l = lowered("ru");
        for xml in [
            render_resw(&l.messages, &en_order()),
            render_stringsdict(&l.messages, &en_order()),
        ] {
            for comment in xml.split("<!--").skip(1) {
                let body = comment.split("-->").next().unwrap_or_default();
                assert!(!body.contains("--"), "{body}");
            }
        }
    }

    #[test]
    fn literal_braces_and_percent_are_escaped() {
        let pieces = lower_text("en", "m", "100% {{x}} {count}").expect("valid");
        let order = vec!["count".to_owned()];
        assert_eq!(apple(&pieces, &order, true), "100%% {x} %lld");
        assert_eq!(apple(&pieces, &order, false), "100%% {x} %@");
        assert_eq!(dotnet(&pieces, &order), "100% {{x}} {0}");
    }

    const RU_HDR: &str = "msgid \"\"\nmsgstr \"Plural-Forms: nplurals=3; plural=(n%10==1 && n%100!=11 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2);\\n\"\n\n";

    fn with_ru(ru_body: &str) -> &'static [Locale] {
        let source: &'static str = Box::leak(format!("{RU_HDR}{ru_body}").into_boxed_str());
        Box::leak(Box::new([
            LOCALES[0],
            Locale {
                source,
                ..LOCALES[1]
            },
        ]))
    }

    #[test]
    fn a_plural_with_the_wrong_number_of_forms_is_rejected() {
        let body = "msgctxt \"session-count\"\nmsgid \"{count} session\"\nmsgid_plural \"{count} sessions\"\nmsgstr[0] \"{count} сессия\"\nmsgstr[1] \"{count} сессии\"\n";
        let err = lower_all(with_ru(body)).err().expect("rejects two forms");
        assert!(err.to_string().contains("nplurals=3"), "{err}");
    }

    #[test]
    fn unknown_ids_and_placeholders_are_rejected() {
        let err = lower_all(with_ru("msgctxt \"nope\"\nmsgid \"x\"\nmsgstr \"y\"\n"))
            .err()
            .expect("rejects an id en lacks");
        assert!(err.to_string().contains("not defined in en"), "{err}");
        let body = "msgctxt \"settings-title\"\nmsgid \"Settings\"\nmsgstr \"{user}\"\n";
        let err = lower_all(with_ru(body)).err().expect("rejects {user}");
        assert!(err.to_string().contains("not a placeholder"), "{err}");
        let body = "msgctxt \"settings-title\"\nmsgid \"Settings\"\nmsgstr \"{0}\"\n";
        let err = lower_all(with_ru(body)).err().expect("rejects {0}");
        assert!(err.to_string().contains("literal braces"), "{err}");
    }
}
