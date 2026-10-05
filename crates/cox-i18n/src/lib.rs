// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox-i18n`: the user-facing strings of cox in gettext catalogs (`.po`),
//! embedded in the binary, and the one place a message id becomes text in the
//! user's language. English (`en`) is the source and default locale; `ru` and
//! `uk` are translations. A message a translation lacks (or leaves empty, or
//! marks fuzzy) resolves from `en`, message by message, so a partial
//! translation is safe to ship.
//!
//! Pure Rust: the catalogs are parsed with `polib` and the `Plural-Forms`
//! rules evaluated by [`plural`], so there is no libintl and nothing to
//! install on macOS or Windows.
//!
//! Its own crate so every surface (TUI, CLI, the desktop apps through
//! `po-export`) reads one set of catalogs without pulling the i18n stack
//! into crates that print nothing. No workspace dependency: it is a leaf.
//!
//! [`export`] lowers the same catalogs to Apple `.strings`/`.stringsdict` and
//! Windows `.resw` files for the native apps (`docs/i18n.md`).

// why: this crate was not under `missing_docs` before the workspace lints;
// drop this allow once its public items are documented.
#![allow(missing_docs)]

pub mod catalog;
pub mod export;
pub mod format;
pub mod plural;

use std::borrow::Cow;
use std::fmt;
use std::sync::OnceLock;

pub use unic_langid::LanguageIdentifier;

use crate::catalog::{Catalog, Entry};

/// One embedded locale.
#[derive(Debug, Clone, Copy)]
pub struct Locale {
    /// BCP 47 language subtag: the catalog name (`po/<code>.po`), the
    /// negotiation target and the Apple `<code>.lproj` name.
    pub code: &'static str,
    /// The Windows resource folder (`Strings/<windows>/Resources.resw`).
    pub windows: &'static str,
    /// The text of `po/<code>.po`.
    pub source: &'static str,
    /// The CLDR plural category of each gettext form, by `msgstr[i]` index.
    pub plural_categories: &'static [&'static str],
    /// The form CLDR `other` uses when `plural_categories` lacks it (ru and
    /// uk: `other` covers only fractions) and the translator wrote no
    /// `# cldr-other:` override. Also used for a non-numeric `count`.
    pub other_form: usize,
}

/// The source and fallback locale: every message id exists here first.
pub const DEFAULT_LOCALE: &str = "en";

/// The product name, substituted for `{brand}` in every message (the
/// gettext stand-in for the Fluent term `-brand-name`).
pub const BRAND_NAME: &str = "Cox";

/// Every shipped locale, the default first. Adding one is a row here plus
/// `po/<code>.po` (`po/README.md`).
pub const LOCALES: &[Locale] = &[
    Locale {
        code: "en",
        windows: "en-US",
        source: include_str!("../po/en.po"),
        plural_categories: &["one", "other"],
        other_form: 1,
    },
    Locale {
        code: "ru",
        windows: "ru-RU",
        source: include_str!("../po/ru.po"),
        plural_categories: &["one", "few", "many"],
        other_form: 1,
    },
    Locale {
        code: "uk",
        windows: "uk-UA",
        source: include_str!("../po/uk.po"),
        plural_categories: &["one", "few", "many"],
        other_form: 1,
    },
];

/// Why a catalog could not be loaded from the embedded sources.
#[derive(Debug, thiserror::Error)]
pub enum I18nError {
    #[error("locale `{code}` is not a valid BCP 47 language identifier")]
    BadCode { code: String },
    #[error("locale `{code}`: {detail}")]
    Parse { code: String, detail: String },
    #[error("locale `{code}`: bad Plural-Forms: {detail}")]
    PluralForms { code: String, detail: String },
    #[error("locale `{code}`: message `{key}` is defined twice")]
    Duplicate { code: String, key: String },
}

/// A message argument.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Int(i64),
    Float(f64),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Str(s) => f.write_str(s),
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) => write!(f, "{x}"),
        }
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Str(s.to_owned())
    }
}
impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Str(s)
    }
}
impl From<&String> for Value {
    fn from(s: &String) -> Self {
        Value::Str(s.clone())
    }
}
macro_rules! int_value {
    ($($t:ty),*) => {$(
        impl From<$t> for Value {
            fn from(v: $t) -> Self {
                Value::Int(i64::try_from(v).unwrap_or(i64::MAX))
            }
        }
    )*};
}
int_value!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);
impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Float(v)
    }
}
impl From<f32> for Value {
    fn from(v: f32) -> Self {
        Value::Float(f64::from(v))
    }
}

/// Named message arguments, filling `{name}` placeholders. The argument
/// named `count` also selects the plural form.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Args {
    values: Vec<(Cow<'static, str>, Value)>,
}

impl Args {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets `name` (replacing an earlier value).
    pub fn set(&mut self, name: impl Into<Cow<'static, str>>, value: impl Into<Value>) {
        let name = name.into();
        let value = value.into();
        match self.values.iter_mut().find(|(n, _)| *n == name) {
            Some(slot) => slot.1 = value,
            None => self.values.push((name, value)),
        }
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.values.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }
}

/// How the `count` argument selects a plural form.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Count {
    /// An integer (the sign dropped: gettext counts are unsigned).
    Whole(u64),
    /// A fraction, a string or no `count` at all: CLDR `other`.
    Other,
}

fn count_of(args: Option<&Args>) -> Count {
    match args.and_then(|a| a.get("count")) {
        Some(Value::Int(i)) => Count::Whole(i.unsigned_abs()),
        Some(Value::Float(x)) if x.is_finite() && x.fract() == 0.0 && x.abs() < 1e19 => {
            // Truncation is exact here: the value is integral and in range.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Count::Whole(x.abs() as u64)
        }
        _ => Count::Other,
    }
}

/// A negotiated chain of catalogs, most preferred first and `en` last.
pub struct Localizer {
    catalogs: Vec<(&'static Locale, Catalog)>,
}

impl Localizer {
    /// Catalogs for `requested` (most preferred first) negotiated against
    /// [`LOCALES`], always ending in [`DEFAULT_LOCALE`].
    pub fn new(requested: &[LanguageIdentifier]) -> Result<Self, I18nError> {
        let catalogs = negotiate(requested)
            .into_iter()
            .map(|locale| Ok((locale, Catalog::parse(locale.code, locale.source)?)))
            .collect::<Result<_, I18nError>>()?;
        Ok(Self { catalogs })
    }

    /// [`Localizer::new`] over raw tags as the OS or env spell them
    /// (`uk_UA.UTF-8`, `ru-RU`); tags that do not parse are skipped.
    pub fn for_tags<S: AsRef<str>>(tags: &[S]) -> Result<Self, I18nError> {
        let requested: Vec<_> = tags.iter().filter_map(|t| parse_tag(t.as_ref())).collect();
        Self::new(&requested)
    }

    /// The user's languages from the env, then the OS ([`requested_languages`]).
    pub fn from_env() -> Result<Self, I18nError> {
        Self::new(&requested_languages())
    }

    /// The negotiated locale codes, most preferred first, `en` last.
    pub fn chain(&self) -> Vec<&'static str> {
        self.catalogs.iter().map(|(l, _)| l.code).collect()
    }

    /// The first locale in the chain with a usable translation of `id`
    /// (`msgctxt`, or `msgid` for an entry without one), formatted; when none
    /// has one, the English source text (`msgid`/`msgid_plural`) of the
    /// entry. `None` when no catalog, `en` included, defines `id`.
    pub fn try_format(&self, id: &str, args: Option<&Args>) -> Option<String> {
        let count = count_of(args);
        let template = self
            .catalogs
            .iter()
            .find_map(|(locale, catalog)| pick(locale, catalog, catalog.get(id)?, count))
            .or_else(|| {
                let entry = self.catalogs.iter().rev().find_map(|(_, c)| c.get(id))?;
                Some(source_text(entry, count))
            })?;
        Some(format::render(template, |name| {
            args.and_then(|a| a.get(name))
                .map(ToString::to_string)
                .or_else(|| (name == "brand").then(|| BRAND_NAME.to_owned()))
        }))
    }

    /// [`Localizer::try_format`], or the id itself when nothing defines it, so
    /// a missing string shows up in the UI as its id instead of blank space.
    pub fn format(&self, id: &str, args: Option<&Args>) -> String {
        self.try_format(id, args).unwrap_or_else(|| id.to_owned())
    }
}

/// The translated form of `entry` for `count` in `locale`, if it has one.
fn pick<'a>(locale: &Locale, catalog: &Catalog, entry: &'a Entry, count: Count) -> Option<&'a str> {
    let forms = entry.translated(catalog.rule.nplurals())?;
    if entry.msgid_plural.is_none() {
        return forms.first().map(String::as_str);
    }
    match count {
        Count::Whole(n) => forms.get(catalog.rule.index(n)).map(String::as_str),
        Count::Other => entry
            .other
            .as_deref()
            .or_else(|| forms.get(locale.other_form).map(String::as_str)),
    }
}

/// The English source of `entry`, with English plural selection.
fn source_text(entry: &Entry, count: Count) -> &str {
    match (&entry.msgid_plural, count) {
        (Some(_), Count::Whole(1)) | (None, _) => &entry.msgid,
        (Some(plural), _) => plural,
    }
}

/// Negotiates `requested` against [`LOCALES`] by language subtag, in request
/// order (so `uk-UA` and `uk_UA.UTF-8` select `uk`), appending
/// [`DEFAULT_LOCALE`] when it is not already in the chain.
pub fn negotiate(requested: &[LanguageIdentifier]) -> Vec<&'static Locale> {
    let mut chain: Vec<&'static Locale> = Vec::new();
    let codes = requested
        .iter()
        .map(|id| id.language.as_str())
        .chain([DEFAULT_LOCALE]);
    for code in codes {
        if let Some(locale) = LOCALES.iter().find(|l| l.code == code)
            && !chain.iter().any(|l| l.code == code)
        {
            chain.push(locale);
        }
    }
    chain
}

/// Parses a tag as POSIX env vars and OS APIs spell it: `uk_UA.UTF-8`,
/// `ru_RU@euro`, `en-US`. `C` and `POSIX` carry no language and give `None`.
pub fn parse_tag(raw: &str) -> Option<LanguageIdentifier> {
    let tag = raw.split(['.', '@']).next()?.trim().replace('_', "-");
    if tag.is_empty() || tag.eq_ignore_ascii_case("c") || tag.eq_ignore_ascii_case("posix") {
        return None;
    }
    tag.parse().ok()
}

/// The user's languages, most preferred first: the POSIX message locale
/// (`LC_ALL`, `LC_MESSAGES`, `LANG`, first one set wins), then the OS UI
/// languages (macOS and Windows preferences; a GUI app gets no `LANG`).
pub fn requested_languages() -> Vec<LanguageIdentifier> {
    requested_from(|name| std::env::var(name).ok(), sys_locale::get_locales())
}

fn requested_from(
    env: impl Fn(&str) -> Option<String>,
    os: impl IntoIterator<Item = String>,
) -> Vec<LanguageIdentifier> {
    let posix = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|name| env(name).filter(|v| !v.is_empty()));
    posix
        .into_iter()
        .chain(os)
        .filter_map(|tag| parse_tag(&tag))
        .collect()
}

/// The process-wide localizer, negotiated from the env and OS on first use.
/// If the embedded catalogs were broken (the tests below rule that out) it
/// degrades to an empty chain, where every message renders as its id.
pub fn global() -> &'static Localizer {
    static GLOBAL: OnceLock<Localizer> = OnceLock::new();
    GLOBAL.get_or_init(|| {
        Localizer::from_env().unwrap_or(Localizer {
            catalogs: Vec::new(),
        })
    })
}

/// `id` in the user's language through [`global`]; see [`Localizer::format`].
pub fn t(id: &str, args: Option<&Args>) -> String {
    global().format(id, args)
}

/// `tr!("id")` or `tr!("id", name = value, …)`: [`t`] with the named arguments
/// filling `{name}` placeholders. Values are anything `Into<Value>`; pass the
/// plural count as a number named `count` so it selects the form.
#[macro_export]
macro_rules! tr {
    ($id:expr $(,)?) => {
        $crate::t($id, None)
    };
    ($id:expr, $($name:ident = $value:expr),+ $(,)?) => {{
        let mut args = $crate::Args::new();
        $(args.set(stringify!($name), $value);)+
        $crate::t($id, Some(&args))
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The template, kept in the crate but only read by tests.
    pub(crate) const POT: &str = include_str!("../po/messages.pot");

    fn localizer(tag: &str) -> Localizer {
        Localizer::for_tags(&[tag]).expect("embedded locales build")
    }

    fn with_count(l: &Localizer, id: &str, count: impl Into<Value>) -> String {
        let mut args = Args::new();
        args.set("count", count);
        args.set("name", "Ivan");
        l.format(id, Some(&args))
    }

    fn sessions(l: &Localizer, count: i64) -> String {
        with_count(l, "session-count", count)
    }

    fn catalog(code: &str) -> Catalog {
        let locale = LOCALES.iter().find(|l| l.code == code).expect("locale");
        Catalog::parse(code, locale.source).expect("parses")
    }

    #[test]
    fn every_embedded_locale_builds() {
        for locale in LOCALES {
            Catalog::parse(locale.code, locale.source).unwrap_or_else(|e| panic!("{e}"));
            locale
                .code
                .parse::<LanguageIdentifier>()
                .unwrap_or_else(|e| panic!("{}: {e}", locale.code));
        }
    }

    #[test]
    fn russian_plurals_pick_one_few_many() {
        let ru = localizer("ru");
        assert_eq!(sessions(&ru, 1), "1 сессия");
        assert_eq!(sessions(&ru, 2), "2 сессии");
        assert_eq!(sessions(&ru, 5), "5 сессий");
        assert_eq!(sessions(&ru, 21), "21 сессия");
    }

    #[test]
    fn ukrainian_plurals_pick_one_few_many() {
        let uk = localizer("uk");
        assert_eq!(sessions(&uk, 1), "1 сесія");
        assert_eq!(sessions(&uk, 2), "2 сесії");
        assert_eq!(sessions(&uk, 5), "5 сесій");
        assert_eq!(sessions(&uk, 21), "21 сесія");
    }

    #[test]
    fn english_plurals_pick_one_other() {
        let en = localizer("en");
        assert_eq!(sessions(&en, 1), "1 session");
        assert_eq!(sessions(&en, 2), "2 sessions");
        assert_eq!(sessions(&en, 21), "21 sessions");
    }

    #[test]
    fn plural_table_for_every_locale() {
        let ns = [0, 1, 2, 5, 11, 21, 22, 25, 111];
        let ru = [
            "сессий",
            "сессия",
            "сессии",
            "сессий",
            "сессий",
            "сессия",
            "сессии",
            "сессий",
            "сессий",
        ];
        let uk = [
            "сесій",
            "сесія",
            "сесії",
            "сесій",
            "сесій",
            "сесія",
            "сесії",
            "сесій",
            "сесій",
        ];
        let ru_files = [
            "файлов",
            "файл",
            "файла",
            "файлов",
            "файлов",
            "файл",
            "файла",
            "файлов",
            "файлов",
        ];
        let uk_files = [
            "файлів",
            "файл",
            "файли",
            "файлів",
            "файлів",
            "файл",
            "файли",
            "файлів",
            "файлів",
        ];
        let (ru_l, uk_l) = (localizer("ru"), localizer("uk"));
        for (i, n) in ns.into_iter().enumerate() {
            assert_eq!(sessions(&ru_l, n), format!("{n} {}", ru[i]));
            assert_eq!(sessions(&uk_l, n), format!("{n} {}", uk[i]));
            assert_eq!(
                with_count(&ru_l, "files-changed", n),
                format!("Ivan изменяет {n} {}", ru_files[i])
            );
            assert_eq!(
                with_count(&uk_l, "files-changed", n),
                format!("Ivan змінює {n} {}", uk_files[i])
            );
        }
        let en = localizer("en");
        assert_eq!(sessions(&en, 0), "0 sessions");
        assert_eq!(sessions(&en, 1), "1 session");
        assert_eq!(sessions(&en, 2), "2 sessions");
        assert_eq!(with_count(&en, "files-changed", 1), "Ivan changed 1 file");
    }

    #[test]
    fn negative_and_fractional_counts() {
        let (ru, uk, en) = (localizer("ru"), localizer("uk"), localizer("en"));
        // gettext counts are unsigned: the sign is dropped for selection only.
        assert_eq!(sessions(&ru, -21), "-21 сессия");
        // Fractions are CLDR `other`: the few form, or a `# cldr-other:` override.
        assert_eq!(with_count(&ru, "session-count", 1.5), "1.5 сессии");
        assert_eq!(
            with_count(&uk, "files-changed", 2.5),
            "Ivan змінює 2.5 файлу"
        );
        assert_eq!(with_count(&en, "session-count", 1.5), "1.5 sessions");
        // An integral float selects like the integer.
        assert_eq!(with_count(&uk, "session-count", 5.0), "5 сесій");
        // A string count is not a number: `other`, as in Fluent.
        assert_eq!(with_count(&ru, "session-count", "5"), "5 сессии");
    }

    #[test]
    fn plural_forms_headers_are_the_standard_rules() {
        let slavic = "nplurals=3; plural=(n%10==1 && n%100!=11 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2);";
        assert_eq!(catalog("en").plural_forms, "nplurals=2; plural=(n != 1);");
        assert_eq!(catalog("ru").plural_forms, slavic);
        assert_eq!(catalog("uk").plural_forms, slavic);
        for locale in LOCALES {
            assert_eq!(
                catalog(locale.code).rule.nplurals(),
                locale.plural_categories.len(),
                "{}",
                locale.code
            );
        }
    }

    #[test]
    fn plural_categories_match_cldr_samples() {
        // CLDR integer samples per category (ru and uk share the rule).
        let samples: &[(&str, &[u64])] = &[
            ("one", &[1, 21, 31, 101, 1001]),
            ("few", &[2, 3, 4, 22, 34, 102]),
            ("many", &[0, 5, 11, 12, 14, 19, 100, 111]),
        ];
        for code in ["ru", "uk"] {
            let (locale, rule) = (
                LOCALES.iter().find(|l| l.code == code).expect("locale"),
                catalog(code).rule,
            );
            for (category, ns) in samples {
                for &n in *ns {
                    assert_eq!(
                        locale.plural_categories[rule.index(n)],
                        *category,
                        "{code} {n}"
                    );
                }
            }
        }
        let en = catalog("en").rule;
        assert_eq!(LOCALES[0].plural_categories[en.index(1)], "one");
        for n in [0, 2, 11, 21] {
            assert_eq!(LOCALES[0].plural_categories[en.index(n)], "other");
        }
    }

    #[test]
    fn unsupported_locale_falls_back_to_english() {
        let de = localizer("de-DE");
        assert_eq!(de.chain(), ["en"]);
        assert_eq!(de.format("settings-title", None), "Settings");
    }

    #[test]
    fn message_missing_from_a_translation_resolves_from_english() {
        for tag in ["ru", "uk"] {
            let l = localizer(tag);
            assert_eq!(l.chain(), [tag, "en"]);
            assert_ne!(l.format("settings-title", None), "Settings");
            // `msgstr ""` in ru.po and uk.po.
            assert_eq!(l.format("send-feedback", None), "Send feedback");
        }
    }

    /// A chain built from inline catalogs, to exercise the fallback rules.
    fn inline_chain(en: &'static str, uk: &'static str) -> Localizer {
        let locale = |code, source| -> &'static Locale {
            Box::leak(Box::new(Locale {
                code,
                windows: "",
                source,
                plural_categories: if code == "en" {
                    &["one", "other"]
                } else {
                    &["one", "few", "many"]
                },
                other_form: 1,
            }))
        };
        let (uk, en) = (locale("uk", uk), locale("en", en));
        Localizer {
            catalogs: vec![
                (uk, Catalog::parse("uk", uk.source).expect("uk")),
                (en, Catalog::parse("en", en.source).expect("en")),
            ],
        }
    }

    const EN_HDR: &str = "msgid \"\"\nmsgstr \"Plural-Forms: nplurals=2; plural=(n != 1);\\n\"\n\n";

    #[test]
    fn fallback_order_is_translation_then_en_msgstr_then_msgid() {
        let en: &'static str = Box::leak(
            format!(
                "{EN_HDR}msgctxt \"a\"\nmsgid \"A source\"\nmsgstr \"A en\"\n\n\
                 msgctxt \"b\"\nmsgid \"B source\"\nmsgstr \"\"\n\n\
                 msgctxt \"c\"\nmsgid \"C source\"\nmsgstr \"C en\"\n\n\
                 msgctxt \"p\"\nmsgid \"{{count}} thing\"\nmsgid_plural \"{{count}} things\"\n\
                 msgstr[0] \"\"\nmsgstr[1] \"\"\n\n\
                 msgid \"Plain msgid key\"\nmsgstr \"\"\n"
            )
            .into_boxed_str(),
        );
        let uk: &'static str = Box::leak(
            "msgid \"\"\nmsgstr \"Plural-Forms: nplurals=3; plural=(n%10==1 && n%100!=11 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2);\\n\"\n\n\
             msgctxt \"a\"\nmsgid \"A source\"\nmsgstr \"\"\n\n\
             #, fuzzy\nmsgctxt \"c\"\nmsgid \"C source\"\nmsgstr \"C fuzzy\"\n\n\
             msgctxt \"p\"\nmsgid \"{count} thing\"\nmsgid_plural \"{count} things\"\n\
             msgstr[0] \"{count} річ\"\nmsgstr[1] \"\"\nmsgstr[2] \"{count} речей\"\n"
                .to_owned()
                .into_boxed_str(),
        );
        let l = inline_chain(en, uk);
        // Empty uk msgstr -> en msgstr.
        assert_eq!(l.format("a", None), "A en");
        // Missing from uk, empty in en -> the msgid.
        assert_eq!(l.format("b", None), "B source");
        // Fuzzy uk -> en.
        assert_eq!(l.format("c", None), "C en");
        // A plural with one empty form falls back as a whole, to en's msgid/msgid_plural.
        let mut args = Args::new();
        args.set("count", 1);
        assert_eq!(l.format("p", Some(&args)), "1 thing");
        args.set("count", 5);
        assert_eq!(l.format("p", Some(&args)), "5 things");
        // An entry without msgctxt is keyed by its msgid.
        assert_eq!(l.format("Plain msgid key", None), "Plain msgid key");
        assert_eq!(l.try_format("zzz", None), None);
    }

    #[test]
    fn missing_key_renders_as_its_id() {
        let ru = localizer("ru");
        assert_eq!(ru.try_format("no-such-message", None), None);
        assert_eq!(ru.format("no-such-message", None), "no-such-message");
    }

    #[test]
    fn os_and_posix_tags_select_the_language() {
        assert_eq!(localizer("uk-UA").chain(), ["uk", "en"]);
        assert_eq!(localizer("uk_UA.UTF-8").chain(), ["uk", "en"]);
        assert_eq!(localizer("ru_RU@euro").chain(), ["ru", "en"]);
        assert_eq!(localizer("C").chain(), ["en"]);
        assert_eq!(localizer("en-GB").chain(), ["en"]);
        let many = Localizer::for_tags(&["de", "uk-UA", "ru", "uk"]).expect("builds");
        assert_eq!(many.chain(), ["uk", "ru", "en"]);
    }

    #[test]
    fn posix_env_wins_over_os_languages() {
        let env = |name: &str| (name == "LANG").then(|| "uk_UA.UTF-8".to_owned());
        let requested = requested_from(env, ["ru-RU".to_owned()]);
        let chain = Localizer::new(&requested).expect("builds").chain();
        assert_eq!(chain, ["uk", "ru", "en"]);
        // LC_ALL beats LANG; an empty value counts as unset.
        let env = |name: &str| match name {
            "LC_ALL" => Some(String::new()),
            "LC_MESSAGES" => Some("ru_RU.UTF-8".to_owned()),
            "LANG" => Some("uk_UA.UTF-8".to_owned()),
            _ => None,
        };
        let chain = Localizer::new(&requested_from(env, []))
            .expect("builds")
            .chain();
        assert_eq!(chain, ["ru", "en"]);
    }

    #[test]
    fn brand_and_variables_are_substituted() {
        let mut args = Args::new();
        args.set("name", "Ivan");
        assert_eq!(
            localizer("uk").format("welcome-user", Some(&args)),
            "Ласкаво просимо до Cox, Ivan!"
        );
        assert_eq!(localizer("ru").format("quit-app", None), "Завершить Cox");
        assert_eq!(localizer("en").format("app-title", None), "Cox");
        // A missing argument stays visible as its placeholder.
        assert_eq!(
            localizer("en").format("welcome-user", None),
            "Welcome to Cox, {name}!"
        );
    }

    #[test]
    fn translations_define_no_id_english_lacks() {
        let en = catalog("en");
        for locale in LOCALES {
            for entry in catalog(locale.code).entries() {
                let source = en
                    .get(&entry.key)
                    .unwrap_or_else(|| panic!("{}: `{}` is not in en", locale.code, entry.key));
                assert_eq!(source.msgid, entry.msgid, "{}: {}", locale.code, entry.key);
                assert_eq!(source.msgid_plural, entry.msgid_plural);
                // Placeholders: only those of the English message (plus {brand}).
                let allowed: Vec<String> = format::placeholders(&source.msgid)
                    .into_iter()
                    .chain(
                        source
                            .msgid_plural
                            .iter()
                            .flat_map(|p| format::placeholders(p)),
                    )
                    .chain(["brand".to_owned()])
                    .collect();
                for text in entry.forms.iter().chain(&entry.other) {
                    format::validate(text).unwrap_or_else(|e| panic!("{}: {e}", locale.code));
                    for v in format::placeholders(text) {
                        assert!(
                            allowed.contains(&v),
                            "{}: `{{{v}}}` in {}",
                            locale.code,
                            entry.key
                        );
                    }
                }
                if entry.msgid_plural.is_some() {
                    let plural = entry.msgid_plural.as_deref().unwrap_or_default();
                    assert!(
                        plural.contains("{count}"),
                        "{}: plural without {{count}}",
                        entry.key
                    );
                    if entry.forms.iter().any(|f| !f.is_empty()) {
                        assert_eq!(
                            entry.forms.len(),
                            locale.plural_categories.len(),
                            "{}",
                            entry.key
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn every_catalog_holds_exactly_the_template_messages() {
        // polib cannot read the template's placeholder header
        // (`nplurals=INTEGER`), so the test swaps in a valid one.
        let pot = POT.replace(
            "nplurals=INTEGER; plural=EXPRESSION;",
            "nplurals=2; plural=(n != 1);",
        );
        let pot = Catalog::parse("pot", &pot).expect("template parses");
        let keys = |c: &Catalog| -> Vec<(String, String, Option<String>)> {
            c.entries()
                .iter()
                .map(|e| (e.key.clone(), e.msgid.clone(), e.msgid_plural.clone()))
                .collect()
        };
        assert!(
            pot.entries()
                .iter()
                .all(|e| e.forms.iter().all(String::is_empty))
        );
        for locale in LOCALES {
            assert_eq!(
                keys(&catalog(locale.code)),
                keys(&pot),
                "{} is out of sync with messages.pot; run `just i18n-update`",
                locale.code
            );
        }
    }

    #[test]
    fn msgfmt_check_accepts_every_catalog() {
        // GNU gettext is optional (`brew install gettext`); without it the
        // test only notes the skip, and `just i18n-check` runs the same
        // commands where it is installed. The template keeps gettext's
        // placeholder header (`nplurals=INTEGER`), which `--check-header`
        // rejects by design, so it gets the format checks only.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("po");
        let run = |tool: &str, args: &[&str]| match std::process::Command::new(tool)
            .args(args)
            .current_dir(&dir)
            .output()
        {
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr);
                assert!(out.status.success(), "{tool} {args:?}: {err}");
                true
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => panic!("{tool}: {e}"),
        };
        if !run(
            "msgfmt",
            &["--check-format", "--output-file=-", "messages.pot"],
        ) {
            eprintln!("msgfmt not installed; skipping");
            return;
        }
        for locale in LOCALES {
            let po = format!("{}.po", locale.code);
            run("msgfmt", &["--check", "--output-file=-", &po]);
            run("msgcmp", &["--use-untranslated", &po, "messages.pot"]);
        }
    }

    #[test]
    fn tr_macro_passes_named_arguments() {
        // The global chain depends on the machine's locale; English is always
        // in it, so check the fallback-only message and argument plumbing.
        assert_eq!(tr!("send-feedback"), global().format("send-feedback", None));
        let text = tr!("session-count", count = 3);
        assert!(text.starts_with('3'), "{text}");
        let text = tr!("files-changed", name = "Ivan", count = 2usize,);
        assert!(text.starts_with("Ivan"), "{text}");
    }
}
