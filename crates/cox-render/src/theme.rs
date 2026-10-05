// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Semantic colour tokens (T24.1): every colour the TUI draws is named here
//! instead of picked ad hoc at the render site. `dark()`/`light()` give
//! ANSI-16 defaults so the app works over plain SSH with no truecolor
//! support; `apply_truecolor` lets a theme file (T24.2) overlay 24-bit
//! values on top without touching the fallback; `mono()` answers `NO_COLOR`
//! by resetting every token so only `Modifier::BOLD`/`DIM` carry hierarchy.
//!
//! T24.2 adds theme *files*: `~/.cox/themes/<name>.toml` beside three
//! built-ins embedded with `include_str!` (`cox-dark`, `cox-light`,
//! `system`). `parse_theme_file` and `catalog` do the reading and parsing
//! (this crate has no rule against local file I/O, unlike `cox-core`);
//! `resolve` turns a `tui.theme` value — `"dark"`/`"light"`/`"auto"`
//! unchanged from T22.6, or a catalog name — into what to draw with,
//! reusing the caller's single `color::detect_dark` read rather than
//! opening a second path to the same answer. A bad file or an unknown name
//! warns and falls back; it never fails the session (untrusted input).

use std::path::Path;

use cox_protocol::types::PermissionMode;
use ratatui::style::Color;
use toml_edit::{DocumentMut, Item};

/// One named colour per role a cell, a modal, the picker or the banner can
/// take. Every render site reads a field here instead of a `Color::`
/// literal, so a theme is one place to change and `NO_COLOR` (`mono`) is one
/// place to blank.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub text: Color,
    pub dim: Color,
    pub accent: Color,
    pub user: Color,
    pub agent: Color,
    pub tool: Color,
    pub ok: Color,
    pub warn: Color,
    pub error: Color,
    pub diff_add: Color,
    pub diff_del: Color,
    pub diff_hunk: Color,
    pub border: Color,
    pub selection: Color,
    pub mode_plan: Color,
    pub mode_auto: Color,
    pub mode_bypass: Color,
}

/// Every `Theme` field's name, in field order: the `[tokens]` keys a theme
/// file may set. `TrueColorOverrides::set`, `Theme::colors` and the theme
/// editor (T46.6) all read this one list (T46.5).
pub const TOKENS: [&str; 17] = [
    "text",
    "dim",
    "accent",
    "user",
    "agent",
    "tool",
    "ok",
    "warn",
    "error",
    "diff_add",
    "diff_del",
    "diff_hunk",
    "border",
    "selection",
    "mode_plan",
    "mode_auto",
    "mode_bypass",
];

/// The banner's alert badge (T4.3's "`danger-full-access` is loud"): black
/// on `Theme::error`'s red in both themes. This one pair is a fixed
/// contrast choice, not a per-theme role, so it lives here as a constant
/// rather than a `Theme` field — the badge stays legible whatever `dark`/
/// `light` picks for everything else.
pub const ALERT_FG: Color = Color::Black;

impl Theme {
    /// Every token's colour, in [`TOKENS`] order.
    pub fn colors(&self) -> [Color; 17] {
        [
            self.text,
            self.dim,
            self.accent,
            self.user,
            self.agent,
            self.tool,
            self.ok,
            self.warn,
            self.error,
            self.diff_add,
            self.diff_del,
            self.diff_hunk,
            self.border,
            self.selection,
            self.mode_plan,
            self.mode_auto,
            self.mode_bypass,
        ]
    }

    /// The tint of the composer prompt in `mode` (T25.2); the default mode
    /// is plain text so only a mode that changes what runs stands out.
    pub fn mode(&self, mode: PermissionMode) -> Color {
        match mode {
            PermissionMode::Default => self.text,
            PermissionMode::Plan => self.mode_plan,
            PermissionMode::Auto => self.mode_auto,
            PermissionMode::Bypass => self.mode_bypass,
        }
    }

    /// The colours this crate used before T24.1, given a name instead of a
    /// literal at each call site — the token values below are exactly what
    /// used to sit inline, so a dark-theme render is unchanged.
    pub fn dark() -> Self {
        Self {
            text: Color::Reset,
            dim: Color::DarkGray,
            accent: Color::Magenta,
            user: Color::Blue,
            agent: Color::Green,
            tool: Color::Cyan,
            ok: Color::Green,
            warn: Color::Yellow,
            error: Color::Red,
            diff_add: Color::Green,
            diff_del: Color::Red,
            diff_hunk: Color::Cyan,
            border: Color::DarkGray,
            selection: Color::Cyan,
            mode_plan: Color::Blue,
            mode_auto: Color::Green,
            mode_bypass: Color::Red,
        }
    }

    /// The same roles over a light background: only the grey-scale tokens
    /// move (`dim`/`border` read as light grey rather than dark). The eight
    /// hues stay put — they are the terminal's own ANSI colours, and a
    /// terminal already adapts them to its own light palette.
    pub fn light() -> Self {
        Self {
            dim: Color::Gray,
            border: Color::Gray,
            ..Self::dark()
        }
    }

    /// `NO_COLOR`: every token resets to the terminal's own colour, so
    /// `Modifier::BOLD`/`DIM` (applied independently at each call site) are
    /// the only hierarchy left — the same intent as `color::Depth::None`,
    /// but decided once up front instead of stripped from a finished frame.
    pub fn mono() -> Self {
        Self {
            text: Color::Reset,
            dim: Color::Reset,
            accent: Color::Reset,
            user: Color::Reset,
            agent: Color::Reset,
            tool: Color::Reset,
            ok: Color::Reset,
            warn: Color::Reset,
            error: Color::Reset,
            diff_add: Color::Reset,
            diff_del: Color::Reset,
            diff_hunk: Color::Reset,
            border: Color::Reset,
            selection: Color::Reset,
            mode_plan: Color::Reset,
            mode_auto: Color::Reset,
            mode_bypass: Color::Reset,
        }
    }

    /// Overlays 24-bit values a theme file (T24.2) supplies on top of the
    /// ANSI-16 defaults; a field left `None` in `overrides` keeps whatever
    /// `self` already had.
    pub fn apply_truecolor(&mut self, overrides: &TrueColorOverrides) {
        if let Some(c) = overrides.text {
            self.text = c;
        }
        if let Some(c) = overrides.dim {
            self.dim = c;
        }
        if let Some(c) = overrides.accent {
            self.accent = c;
        }
        if let Some(c) = overrides.user {
            self.user = c;
        }
        if let Some(c) = overrides.agent {
            self.agent = c;
        }
        if let Some(c) = overrides.tool {
            self.tool = c;
        }
        if let Some(c) = overrides.ok {
            self.ok = c;
        }
        if let Some(c) = overrides.warn {
            self.warn = c;
        }
        if let Some(c) = overrides.error {
            self.error = c;
        }
        if let Some(c) = overrides.diff_add {
            self.diff_add = c;
        }
        if let Some(c) = overrides.diff_del {
            self.diff_del = c;
        }
        if let Some(c) = overrides.diff_hunk {
            self.diff_hunk = c;
        }
        if let Some(c) = overrides.border {
            self.border = c;
        }
        if let Some(c) = overrides.selection {
            self.selection = c;
        }
        if let Some(c) = overrides.mode_plan {
            self.mode_plan = c;
        }
        if let Some(c) = overrides.mode_auto {
            self.mode_auto = c;
        }
        if let Some(c) = overrides.mode_bypass {
            self.mode_bypass = c;
        }
    }
}

/// 24-bit overrides a theme file (T24.2) supplies; every field is optional
/// so a theme can redefine only the tokens it cares about and fall back to
/// the ANSI-16 default (`Theme::dark`/`light`) for the rest.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TrueColorOverrides {
    pub text: Option<Color>,
    pub dim: Option<Color>,
    pub accent: Option<Color>,
    pub user: Option<Color>,
    pub agent: Option<Color>,
    pub tool: Option<Color>,
    pub ok: Option<Color>,
    pub warn: Option<Color>,
    pub error: Option<Color>,
    pub diff_add: Option<Color>,
    pub diff_del: Option<Color>,
    pub diff_hunk: Option<Color>,
    pub border: Option<Color>,
    pub selection: Option<Color>,
    pub mode_plan: Option<Color>,
    pub mode_auto: Option<Color>,
    pub mode_bypass: Option<Color>,
}

impl TrueColorOverrides {
    /// Sets the field named `token` (the `[tokens]` key of a theme file);
    /// an unrecognised name is ignored rather than failing the whole file —
    /// the same fail-open contract `Glyphs::set` gives `[tui.icons]`.
    pub fn set(&mut self, token: &str, color: Color) {
        let Some(i) = TOKENS.iter().position(|t| *t == token) else {
            return;
        };
        if let Some(slot) = self.slots().into_iter().nth(i) {
            *slot = Some(color);
        }
    }

    /// Every field, in [`TOKENS`] order.
    fn slots(&mut self) -> [&mut Option<Color>; 17] {
        [
            &mut self.text,
            &mut self.dim,
            &mut self.accent,
            &mut self.user,
            &mut self.agent,
            &mut self.tool,
            &mut self.ok,
            &mut self.warn,
            &mut self.error,
            &mut self.diff_add,
            &mut self.diff_del,
            &mut self.diff_hunk,
            &mut self.border,
            &mut self.selection,
            &mut self.mode_plan,
            &mut self.mode_auto,
            &mut self.mode_bypass,
        ]
    }
}

/// A parsed `~/.cox/themes/<name>.toml` or built-in (T24.2 step 1): the
/// dark and light truecolor overlays `[tokens]` supplies (each entry names
/// what it changes; anything else keeps the ANSI-16 default), the syntect
/// theme `syntax` names (if any, overriding `tui.syntax_theme`), and the
/// background `variant` the file pins itself to, if it pins one at all — a
/// user file that omits it follows the terminal's own OSC 11 read, exactly
/// like `tui.theme = "auto"`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThemeFile {
    pub dark: TrueColorOverrides,
    pub light: TrueColorOverrides,
    pub syntax: Option<String>,
    pub variant: Option<bool>,
}

impl ThemeFile {
    /// Builds the `Theme` this file yields for `dark`'s variant: the
    /// ANSI-16 baseline overlaid with whichever half of `[tokens]` matches.
    pub fn theme(&self, dark: bool) -> Theme {
        let mut theme = if dark { Theme::dark() } else { Theme::light() };
        theme.apply_truecolor(if dark { &self.dark } else { &self.light });
        theme
    }
}

/// A theme file's TOML could not even be parsed (bad syntax); the caller
/// skips the file (`catalog`) or falls back to the pre-T24.2 default
/// (`resolve`) rather than failing the session.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid TOML: {0}")]
pub struct ThemeFileError(String);

/// Parses one theme file's TOML with the same crate `cox config set`
/// already edits config with (`toml_edit`), so a future `cox theme edit`
/// could reuse it too. A `[tokens]` entry missing `dark`/`light`, or a
/// colour string `parse_color` does not understand, is simply not set —
/// only a TOML syntax error is fatal to the whole file.
pub fn parse_theme_file(src: &str) -> Result<ThemeFile, ThemeFileError> {
    let doc: DocumentMut = src
        .parse()
        .map_err(|e: toml_edit::TomlError| ThemeFileError(e.message().to_string()))?;
    let mut file = ThemeFile {
        variant: doc
            .get("variant")
            .and_then(Item::as_str)
            .and_then(|s| match s {
                "dark" => Some(true),
                "light" => Some(false),
                _ => None,
            }),
        syntax: doc.get("syntax").and_then(Item::as_str).map(str::to_string),
        ..ThemeFile::default()
    };
    if let Some(tokens) = doc.get("tokens").and_then(Item::as_table_like) {
        for (name, item) in tokens.iter() {
            let Some(spec) = item.as_table_like() else {
                continue;
            };
            if let Some(c) = spec
                .get("dark")
                .and_then(Item::as_str)
                .and_then(parse_color)
            {
                file.dark.set(name, c);
            }
            if let Some(c) = spec
                .get("light")
                .and_then(Item::as_str)
                .and_then(parse_color)
            {
                file.light.set(name, c);
            }
        }
    }
    Ok(file)
}

/// Sets `[tokens.<token>]`'s `dark` (or `light`) colour in a theme file's
/// source (T46.5), keeping comments, formatting and every other key; the
/// token's entry (`<token> = { dark = … }`) is created when missing. A token not in
/// [`TOKENS`], TOML that does not parse, or a `tokens`/token entry that is
/// not a table is an error, never a panic: the source may be a user file.
pub fn set_token(
    src: &str,
    token: &str,
    dark: bool,
    color: Color,
) -> Result<String, ThemeFileError> {
    if !TOKENS.contains(&token) {
        return Err(ThemeFileError(format!("unknown token `{token}`")));
    }
    let mut doc: DocumentMut = src
        .parse()
        .map_err(|e: toml_edit::TomlError| ThemeFileError(e.message().to_string()))?;
    let not_a_table = |key: &str| ThemeFileError(format!("`{key}` is not a table"));
    let tokens = doc
        .entry("tokens")
        .or_insert(Item::Table(toml_edit::Table::new()))
        .as_table_like_mut()
        .ok_or_else(|| not_a_table("tokens"))?;
    // Inline, as the built-ins write each token, and valid whether
    // `tokens` itself is a `[tokens]` table or an inline one.
    let spec = tokens
        .entry(token)
        .or_insert(Item::Value(toml_edit::Value::InlineTable(
            toml_edit::InlineTable::new(),
        )))
        .as_table_like_mut()
        .ok_or_else(|| not_a_table(token))?;
    let key = if dark { "dark" } else { "light" };
    let mut value = toml_edit::Value::from(format_color(color));
    if let Some(old) = spec.get(key).and_then(Item::as_value) {
        *value.decor_mut() = old.decor().clone();
    }
    spec.insert(key, Item::Value(value));
    Ok(doc.to_string())
}

/// The embedded source of a built-in theme, so an edit of one starts from
/// its own text (T46.5).
pub fn builtin_source(name: &str) -> Option<&'static str> {
    BUILT_IN_THEMES
        .iter()
        .find(|(builtin, _)| *builtin == name)
        .map(|(_, src)| *src)
}

/// [`parse_color`]'s inverse: `#rrggbb`, a bare index, or one of the
/// sixteen names (plus `reset`) in the spelling `parse_color` reads.
pub fn format_color(color: Color) -> String {
    let name = match color {
        Color::Rgb(r, g, b) => return format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(i) => return i.to_string(),
        Color::Reset => "reset",
        Color::Black => "black",
        Color::Red => "red",
        Color::Green => "green",
        Color::Yellow => "yellow",
        Color::Blue => "blue",
        Color::Magenta => "magenta",
        Color::Cyan => "cyan",
        Color::White => "white",
        Color::Gray => "gray",
        Color::DarkGray => "darkgray",
        Color::LightRed => "lightred",
        Color::LightGreen => "lightgreen",
        Color::LightYellow => "lightyellow",
        Color::LightBlue => "lightblue",
        Color::LightMagenta => "lightmagenta",
        Color::LightCyan => "lightcyan",
    };
    name.to_string()
}

/// A `[tokens]` colour: `#rrggbb` truecolor, a bare `0`-`255` ANSI index (so
/// a theme like the `system` built-in can name the terminal's own palette
/// slot instead of a fixed hex), or one of the sixteen ANSI colour names.
pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() != 6 {
            return None;
        }
        let byte = |i: usize| -> Option<u8> { u8::from_str_radix(hex.get(i..i + 2)?, 16).ok() };
        return Some(Color::Rgb(byte(0)?, byte(2)?, byte(4)?));
    }
    if let Ok(index) = s.parse::<u8>() {
        return Some(Color::Indexed(index));
    }
    Some(match s.to_ascii_lowercase().as_str() {
        "reset" => Color::Reset,
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::White,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        _ => return None,
    })
}

/// The built-ins (T24.2 step 2, T29.2's daltonized pair), embedded so
/// `/theme` always has something to offer even with an empty
/// `~/.cox/themes/`.
pub const BUILT_IN_THEMES: &[(&str, &str)] = &[
    ("cox-dark", include_str!("../assets/themes/cox-dark.toml")),
    ("cox-light", include_str!("../assets/themes/cox-light.toml")),
    (
        "cox-dark-daltonized",
        include_str!("../assets/themes/cox-dark-daltonized.toml"),
    ),
    (
        "cox-light-daltonized",
        include_str!("../assets/themes/cox-light-daltonized.toml"),
    ),
    ("system", include_str!("../assets/themes/system.toml")),
];

/// Every colour theme `/theme` can offer: the built-ins, then every
/// `<name>.toml` in `dir` (`~/.cox/themes/`) under its file stem. `dir` not
/// existing is not a warning, just an empty user set; a user file whose
/// TOML `parse_theme_file` rejects is skipped the same way — the picker
/// simply does not offer it.
pub fn catalog(dir: &Path) -> Vec<(String, ThemeFile)> {
    let mut out: Vec<(String, ThemeFile)> = BUILT_IN_THEMES
        .iter()
        .filter_map(|(name, src)| parse_theme_file(src).ok().map(|f| ((*name).to_string(), f)))
        .collect();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(file) = parse_theme_file(&src) {
            out.push((stem.to_string(), file));
        }
    }
    out
}

/// `.tmTheme` files in `dir` (T24.2 step 4): the names `/theme` lists under
/// a `syntax: ` prefix. A missing `dir`, or any file `syntect` cannot
/// parse, yields an empty list rather than an error — the picker just does
/// not offer a syntax theme that session.
pub fn tm_theme_names(dir: &Path) -> Vec<String> {
    syntect::highlighting::ThemeSet::load_from_folder(dir)
        .map(|set| set.themes.into_keys().collect())
        .unwrap_or_default()
}

/// What `tui.theme` decided (T24.2 step 3): the colours to draw with,
/// whether the background reads as dark, the syntect theme it names (if
/// any), and a warning to surface when the name could not be honoured.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub dark: bool,
    pub theme: Theme,
    pub syntax: Option<String>,
    pub warning: Option<String>,
}

/// Resolves `tui.theme` into `Resolved`. `"light"`/`"dark"` are an explicit
/// choice and never query the terminal; `"auto"` and any name found in
/// `catalog` reuse `background_dark` — T22.6's single OSC 11 read, done
/// once by the caller before raw mode — rather than opening a second path
/// to the same answer. An unrecognised name warns and falls back to the
/// pre-T24.2 default (dark) exactly as an unknown value already did.
pub fn resolve(
    name: &str,
    background_dark: Option<bool>,
    catalog: &[(String, ThemeFile)],
) -> Resolved {
    match name {
        "light" => Resolved {
            dark: false,
            theme: Theme::light(),
            syntax: None,
            warning: None,
        },
        "dark" => Resolved {
            dark: true,
            theme: Theme::dark(),
            syntax: None,
            warning: None,
        },
        "auto" => {
            let dark = background_dark.unwrap_or(true);
            Resolved {
                dark,
                theme: if dark { Theme::dark() } else { Theme::light() },
                syntax: None,
                warning: None,
            }
        }
        other => match catalog.iter().find(|(n, _)| n == other) {
            Some((_, file)) => {
                let dark = file
                    .variant
                    .unwrap_or_else(|| background_dark.unwrap_or(true));
                Resolved {
                    dark,
                    theme: file.theme(dark),
                    syntax: file.syntax.clone(),
                    warning: None,
                }
            }
            None => Resolved {
                dark: true,
                theme: Theme::dark(),
                syntax: None,
                warning: Some(format!("unknown tui.theme {other:?}; using the default")),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_resets_every_token() {
        let mono = Theme::mono();
        assert_eq!(mono.text, Color::Reset);
        assert_eq!(mono.error, Color::Reset);
        assert_eq!(mono.mode_bypass, Color::Reset);
    }

    #[test]
    fn dark_and_light_share_hues_but_not_greys() {
        assert_eq!(Theme::dark().error, Theme::light().error);
        assert_eq!(Theme::dark().selection, Theme::light().selection);
        assert_ne!(Theme::dark().dim, Theme::light().dim);
        assert_ne!(Theme::dark().border, Theme::light().border);
    }

    #[test]
    fn apply_truecolor_overlays_only_the_given_fields() {
        let mut theme = Theme::dark();
        let overrides = TrueColorOverrides {
            error: Some(Color::Rgb(220, 20, 60)),
            ..Default::default()
        };
        theme.apply_truecolor(&overrides);
        assert_eq!(theme.error, Color::Rgb(220, 20, 60));
        assert_eq!(
            theme.tool,
            Theme::dark().tool,
            "a field left `None` keeps its default"
        );
    }

    /// T24.1's grep test: every non-test line outside `theme.rs`/`color.rs`
    /// picks a colour through a `Theme` token, never a `Color::` literal.
    /// `svg.rs` (already-resolved `Buffer` → CSS) and `markdown.rs` (syntect's
    /// own 24-bit syntax colours, T24.3's territory) convert a colour someone
    /// else chose rather than choosing a UI one, so both are exempt like
    /// `color.rs` itself.
    #[test]
    fn no_color_literal_outside_theme() {
        let src = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
        // `link.rs` holds a sentinel the terminal never sees, not a colour.
        let exempt = ["theme.rs", "color.rs", "svg.rs", "markdown.rs", "link.rs"];
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(src).expect("read cox-tui/src") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            if exempt.contains(&name.as_str()) {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read source file");
            // Tests may compare against a literal colour freely, same as the
            // crate's `unwrap`/`expect` convention; only non-test code must
            // route through `Theme`.
            for line in text.lines().take_while(|l| l.trim() != "#[cfg(test)]") {
                if line.contains("Color::") {
                    offenders.push(format!("{name}: {}", line.trim()));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "Color:: literal outside theme.rs/color.rs: {offenders:#?}"
        );
    }

    /// T24.2's schema (`plan.md` step 1): a token's `dark`/`light` hex
    /// overlays the right variant and nothing else, a `variant` pin forces
    /// which half applies regardless of the caller's own background, and a
    /// token the file never mentions keeps the ANSI-16 default.
    #[test]
    fn theme_file_round_trips() {
        let src = r##"
variant = "dark"
syntax = "InspiredGitHub"

[tokens]
accent = { dark = "#7aa2f7", light = "#2e5aac" }
error = { dark = "#f7768e" }
system = { dark = "4" }
"##;
        let file = parse_theme_file(src).expect("valid theme file");
        assert_eq!(file.variant, Some(true));
        assert_eq!(file.syntax.as_deref(), Some("InspiredGitHub"));

        let dark = file.theme(true);
        assert_eq!(dark.accent, Color::Rgb(0x7a, 0xa2, 0xf7));
        assert_eq!(dark.error, Color::Rgb(0xf7, 0x76, 0x8e));

        let light = file.theme(false);
        assert_eq!(
            light.accent,
            Color::Rgb(0x2e, 0x5a, 0xac),
            "the `light` half of the same token"
        );
        assert_eq!(
            light.error,
            Theme::light().error,
            "no `light` override for this token: falls back to the built-in"
        );
    }

    #[test]
    fn theme_file_rejects_bad_toml_but_not_an_unknown_token() {
        assert!(parse_theme_file("[tokens\n").is_err());
        // An unrecognised `[tokens]` name (a typo, a future token) is
        // simply not set — the rest of the file still parses.
        let file = parse_theme_file("[tokens]\nnope = { dark = \"#ffffff\" }\n")
            .expect("unknown token name is not a parse error");
        assert_eq!(file.theme(true), Theme::dark());
    }

    #[test]
    fn parse_color_reads_hex_ansi_index_and_names() {
        assert_eq!(parse_color("#7aa2f7"), Some(Color::Rgb(0x7a, 0xa2, 0xf7)));
        assert_eq!(parse_color("9"), Some(Color::Indexed(9)));
        assert_eq!(parse_color("cyan"), Some(Color::Cyan));
        assert_eq!(parse_color("not-a-colour"), None);
        assert_eq!(parse_color("#zzzzzz"), None);
        assert_eq!(parse_color("#fff"), None);
    }

    /// Built-ins must themselves parse — a typo here would silently drop a
    /// whole preset from `/theme` rather than fail a build.
    #[test]
    fn every_built_in_theme_parses() {
        for (name, src) in BUILT_IN_THEMES {
            assert!(parse_theme_file(src).is_ok(), "{name} failed to parse");
        }
    }

    #[test]
    fn catalog_lists_built_ins_and_falls_back_when_the_user_dir_is_missing() {
        let names: Vec<_> = catalog(Path::new("/does/not/exist/themes"))
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(
            names,
            [
                "cox-dark",
                "cox-light",
                "cox-dark-daltonized",
                "cox-light-daltonized",
                "system"
            ]
        );
    }

    #[test]
    fn catalog_reads_a_user_file_under_its_stem() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("nord.toml"),
            "[tokens]\naccent = { dark = \"#88c0d0\" }\n",
        )
        .expect("write user theme");
        let found = catalog(dir.path());
        let (_, file) = found
            .iter()
            .find(|(n, _)| n == "nord")
            .expect("nord.toml is offered under its stem");
        assert_eq!(file.theme(true).accent, Color::Rgb(0x88, 0xc0, 0xd0));
    }

    #[test]
    fn resolve_light_and_dark_never_query_the_background() {
        let light = resolve("light", None, &[]);
        assert!(!light.dark);
        let dark = resolve("dark", None, &[]);
        assert!(dark.dark);
    }

    #[test]
    fn resolve_auto_and_a_named_theme_share_the_one_background_read() {
        let auto = resolve("auto", Some(false), &[]);
        assert!(!auto.dark, "auto follows the caller's OSC 11 read");

        let catalog = vec![("nord".to_string(), ThemeFile::default())];
        let unpinned = resolve("nord", Some(false), &catalog);
        assert!(
            !unpinned.dark,
            "a theme with no `variant` follows the same read auto does"
        );
    }

    #[test]
    fn resolve_unknown_name_warns_and_falls_back_to_dark() {
        let resolved = resolve("no-such-theme", Some(false), &[]);
        assert!(resolved.dark);
        assert_eq!(resolved.theme, Theme::dark());
        assert!(resolved.warning.is_some());
    }

    /// The literal minimum a `.tmTheme` (a plist) needs to parse: a
    /// `settings` array whose first item has no `scope`, meaning it is the
    /// theme's global colours rather than one scope's.
    const FIXTURE_TMTHEME: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>name</key>
    <string>Cox Fixture</string>
    <key>settings</key>
    <array>
        <dict>
            <key>settings</key>
            <dict>
                <key>background</key>
                <string>#1E1E1E</string>
                <key>foreground</key>
                <string>#D4D4D4</string>
            </dict>
        </dict>
    </array>
</dict>
</plist>
"#;

    #[test]
    fn tmtheme_in_themes_dir_is_listed() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("cox-fixture.tmTheme"), FIXTURE_TMTHEME)
            .expect("write fixture");
        let names = tm_theme_names(dir.path());
        assert_eq!(names, vec!["cox-fixture".to_string()]);
    }

    #[test]
    fn tmtheme_missing_dir_is_empty_not_an_error() {
        assert!(tm_theme_names(Path::new("/does/not/exist/themes")).is_empty());
    }

    /// T46.5: `set`, `colors` and `TOKENS` agree on every token, so the one
    /// list really is the struct's field order.
    #[test]
    fn tokens_name_every_theme_field_in_order() {
        for (i, token) in TOKENS.iter().enumerate() {
            let mut overrides = TrueColorOverrides::default();
            let color = Color::Indexed(u8::try_from(i).expect("17 tokens"));
            overrides.set(token, color);
            let mut theme = Theme::mono();
            theme.apply_truecolor(&overrides);
            let colors = theme.colors();
            assert_eq!(colors[i], color, "{token}");
            assert_eq!(colors.iter().filter(|c| **c == color).count(), 1, "{token}");
        }
    }

    #[test]
    fn set_token_keeps_comments_and_other_tokens() {
        let src = builtin_source("cox-dark").expect("a built-in");
        let out = set_token(src, "accent", true, Color::Rgb(1, 2, 3)).expect("set");
        assert!(out.starts_with("# T24.2 built-in"), "{out}");
        assert!(out.contains("variant = \"dark\""), "{out}");
        assert!(out.contains("#010203"), "{out}");
        assert!(
            !out.contains("dark = \"#7aa2f7\", light = \"#2e5aac\" }\nuser"),
            "{out}"
        );
        let (before, after) = (
            parse_theme_file(src).expect("parse"),
            parse_theme_file(&out).expect("parse"),
        );
        assert_eq!(after.dark.accent, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(
            after.light.accent, before.light.accent,
            "the other variant stays"
        );
        assert_eq!(after.dark.error, before.dark.error, "other tokens stay");
        assert_eq!(after.syntax, before.syntax);
        // Only that one value changed: everything else is byte-identical.
        assert_eq!(out.lines().count(), src.lines().count());
        let changed: Vec<_> = src
            .lines()
            .zip(out.lines())
            .filter(|(a, b)| a != b)
            .collect();
        assert_eq!(changed.len(), 1, "{changed:?}");
    }

    #[test]
    fn set_token_round_trips_through_parse() {
        let mut src = String::from("# mine\n");
        for (i, token) in TOKENS.iter().enumerate() {
            let color = Color::Indexed(u8::try_from(i).expect("17 tokens"));
            src = set_token(&src, token, false, color).expect("set");
            src = set_token(&src, token, true, Color::Rgb(0, 0, 10 * i as u8)).expect("set");
        }
        assert!(src.contains("# mine"), "the comment survives: {src}");
        let file = parse_theme_file(&src).expect("parse");
        let (dark, light) = (file.theme(true).colors(), file.theme(false).colors());
        for i in 0..TOKENS.len() {
            assert_eq!(dark[i], Color::Rgb(0, 0, 10 * i as u8), "{}", TOKENS[i]);
            assert_eq!(light[i], Color::Indexed(i as u8), "{}", TOKENS[i]);
        }
    }

    #[test]
    fn format_color_inverts_parse_color() {
        let mut colors = vec![
            Color::Reset,
            Color::Black,
            Color::Red,
            Color::Green,
            Color::Yellow,
            Color::Blue,
            Color::Magenta,
            Color::Cyan,
            Color::White,
            Color::Gray,
            Color::DarkGray,
            Color::LightRed,
            Color::LightGreen,
            Color::LightYellow,
            Color::LightBlue,
            Color::LightMagenta,
            Color::LightCyan,
            Color::Rgb(0, 0, 0),
            Color::Rgb(0xab, 0xcd, 0xef),
        ];
        colors.extend((0..=255).map(Color::Indexed));
        for color in colors {
            assert_eq!(parse_color(&format_color(color)), Some(color), "{color:?}");
        }
        assert_eq!(format_color(Color::Rgb(0xab, 0xcd, 0xef)), "#abcdef");
    }

    #[test]
    fn set_token_rejects_unknown_token() {
        let err = set_token("", "backgroundd", true, Color::Red).expect_err("unknown");
        assert!(err.to_string().contains("backgroundd"), "{err}");
        assert!(set_token("tokens = 1\n", "text", true, Color::Red).is_err());
        assert!(set_token("[tokens]\ntext = \"red\"\n", "text", true, Color::Red).is_err());
        assert!(set_token("not toml [", "text", true, Color::Red).is_err());
    }

    #[test]
    fn builtin_source_names_every_built_in() {
        for (name, src) in BUILT_IN_THEMES {
            assert_eq!(builtin_source(name), Some(*src));
        }
        assert_eq!(builtin_source("mine"), None);
    }
}
