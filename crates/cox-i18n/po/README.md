# Catalogs

GNU gettext catalogs for `cox-i18n`. `messages.pot` is the template (every
message, English, no translations); `en.po` is the source and fallback
locale; `ru.po` and `uk.po` are translations. See `docs/i18n.md` for the full
guide, conventions and the export.

- Ids are `msgctxt`; `msgid` is the English text.
- Placeholders are `{name}` (`#, python-brace-format`); `{{`/`}}` are literal
  braces; `{brand}` is the product name, filled in by the crate.
- Plurals select on `{count}`; Russian and Ukrainian forms are `[0]` one,
  `[1]` few, `[2]` many; `# cldr-other: <text>` gives CLDR `other`
  (fractions) where it differs from the few form.
- Do not start a file with a bare `#` line: the parser (`gettext-catalog`,
  which uses polib) rejects it.

## Changing strings

1. Edit `messages.pot`.
2. `just i18n-update` merges it into every `.po` (`msgmerge --update`).
3. Translate the new, empty or `fuzzy` entries; fill `en.po` too.
4. `just i18n-check` (`msgfmt --check`, `msgcmp`) and
   `cargo test -p cox-i18n`.

GNU gettext is only needed for these steps (`brew install gettext`); the
runtime reads the `.po` files itself.

## Adding a locale

1. `msginit --input=messages.pot --locale=<code> --no-translator
   --output-file=<code>.po`, with `<code>` the language subtag (`de`, `pl`);
   check the `Plural-Forms` it writes.
2. Translate. What you leave empty resolves from `en`, so a partial
   translation is safe to ship.
3. Register it in `LOCALES` in `crates/cox-i18n/src/lib.rs` (code,
   `include_str!`, Windows folder such as `de-DE`, the CLDR category of each
   form, and the form CLDR `other` uses).
4. `just i18n-check`, `cargo test -p cox-i18n` and `just i18n-export`.
