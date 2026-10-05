// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Keybindings (T25.5): `commands::KEYMAP` as the live table `state` resolves
//! keys through, rebindable from `~/.cox/keybindings.toml` and seeded from
//! Claude Code's `keybindings.json`. Separate from `commands` (the default
//! table and its words) so parsing and precedence are tested without a
//! `State`, and so `doctor` can list conflicts without a terminal.

use cox_protocol::plugin::KeyDecl;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::commands::{Context, KEYMAP};

macro_rules! actions {
    ($($variant:ident = $id:literal),* $(,)?) => {
        /// The `KEYMAP` actions `state` dispatches; each id is the table's
        /// action column and a `keybindings.toml` key. `@`, `/`, `Ctrl+R`
        /// and the modal rows stay with the composer and the modals.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Action { $($variant),* }

        impl Action {
            pub const ALL: &[Action] = &[$(Action::$variant),*];

            pub fn id(self) -> &'static str {
                match self { $(Action::$variant => $id),* }
            }
        }
    };
}

actions! {
    Send = "send",
    Newline = "newline",
    SendNow = "send.now",
    Interrupt = "interrupt",
    ModeCycle = "mode.cycle",
    Transcript = "transcript",
    Help = "help",
    Thinking = "thinking",
    Expand = "expand",
    Diff = "diff",
    // T33.25, PL§8: arms the next key as `<leader> <key>` for a plugin.
    PluginLeader = "plugin.leader",
    Background = "background",
    Unqueue = "unqueue",
    Quit = "quit",
    // `y` (T23.4): the last transcript cell still held, as plain text.
    Copy = "copy",
    // `Shift+Y` (T23.4): the whole transcript still held.
    CopyAll = "copy.all",
    // T54.6: push-to-talk; `[voice] key` rebinds it.
    Voice = "voice",
}

impl Action {
    pub fn from_id(id: &str) -> Option<Action> {
        Action::ALL.iter().copied().find(|a| a.id() == id)
    }

    /// Claude Code's name for the same action; Claude has none that opens
    /// a help overlay.
    fn from_claude(name: &str) -> Option<Action> {
        Some(match name {
            "chat:submit" => Action::Send,
            "chat:newline" => Action::Newline,
            "chat:sendNow" => Action::SendNow,
            "chat:cancel" => Action::Interrupt,
            "chat:cycleMode" => Action::ModeCycle,
            "app:toggleTranscript" => Action::Transcript,
            "task:background" => Action::Background,
            "app:exit" => Action::Quit,
            _ => return None,
        })
    }

    /// The contexts the action's default rows are in; a rebinding keeps them.
    fn contexts(self) -> Vec<Context> {
        Context::ALL
            .into_iter()
            .filter(|c| KEYMAP.iter().any(|(_, a, x)| *a == self.id() && x == c))
            .collect()
    }
}

/// One key in one context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub key: KeyCode,
    pub modifiers: KeyModifiers,
    pub context: Context,
}

#[derive(Debug, Clone, PartialEq)]
struct Row {
    /// The key as the hints print it.
    shown: String,
    binding: Binding,
    action: &'static str,
    /// Set by a user file rather than `KEYMAP`.
    user: bool,
}

const MODS: KeyModifiers = KeyModifiers::CONTROL
    .union(KeyModifiers::ALT)
    .union(KeyModifiers::SHIFT)
    .union(KeyModifiers::SUPER);

/// Terminals disagree on how a shifted key arrives — `Shift+Tab` is
/// `BackTab`, `?` may or may not carry `SHIFT` — so both sides of a
/// comparison go through here.
fn normalize(key: KeyCode, modifiers: KeyModifiers) -> (KeyCode, KeyModifiers) {
    let modifiers = modifiers & MODS;
    match key {
        KeyCode::BackTab => (KeyCode::Tab, modifiers | KeyModifiers::SHIFT),
        KeyCode::Char(c) => (KeyCode::Char(c), modifiers - KeyModifiers::SHIFT),
        other => (other, modifiers),
    }
}

/// `"ctrl+enter"`, `"shift+tab"`, `"alt+m"`, `"?"`: modifiers, then one
/// key, any case. `None` for anything else, chords included.
pub fn parse(text: &str) -> Option<(KeyCode, KeyModifiers)> {
    let lower = text.trim().to_ascii_lowercase();
    let (mods, key) = lower.rsplit_once('+').unwrap_or(("", &lower));
    let mut modifiers = KeyModifiers::NONE;
    for m in mods.split('+').filter(|m| !m.is_empty()) {
        modifiers |= match m {
            "ctrl" | "control" => KeyModifiers::CONTROL,
            "alt" | "opt" | "option" | "meta" => KeyModifiers::ALT,
            "shift" => KeyModifiers::SHIFT,
            "cmd" | "command" | "super" => KeyModifiers::SUPER,
            _ => return None,
        };
    }
    let code = match key {
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "space" => KeyCode::Char(' '),
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        _ => match (key.chars().next(), key.chars().count()) {
            (Some(c), 1) if c.is_whitespace() => return None,
            (Some(c), 1) if modifiers.contains(KeyModifiers::SHIFT) => {
                KeyCode::Char(c.to_ascii_uppercase())
            }
            (Some(c), 1) => KeyCode::Char(c),
            _ => match key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
                Some(n @ 1..=12) => KeyCode::F(n),
                _ => return None,
            },
        },
    };
    Some(normalize(code, modifiers))
}

/// `ctrl+enter` → `Ctrl+Enter`, the way `KEYMAP` spells keys.
fn shown(text: &str) -> String {
    let parts: Vec<&str> = text.trim().split('+').collect();
    let last = parts.len().saturating_sub(1);
    let word = |(i, p): (usize, &&str)| {
        let mut chars = p.chars();
        match chars.next() {
            Some(c) if p.len() > 1 || (i == last && i > 0) => {
                c.to_uppercase().chain(chars).collect::<String>()
            }
            _ => p.to_string(),
        }
    };
    parts
        .iter()
        .enumerate()
        .map(word)
        .collect::<Vec<_>>()
        .join("+")
}

/// One plugin's key under the leader (T33.25, PL§8): a separate table from
/// `rows`, since its `plugin`/`name` are runtime strings, not one of the
/// closed `Action`s `rows` binds — the key never resolves through
/// `resolve`, only through `resolve_plugin_key` once the leader armed it.
#[derive(Debug, Clone, PartialEq)]
struct PluginKeyRow {
    shown: String,
    binding: (KeyCode, KeyModifiers),
    plugin: String,
    name: String,
}

/// Every key the TUI knows, in `KEYMAP` order with user keys added.
#[derive(Debug, Clone, PartialEq)]
pub struct Keymap {
    rows: Vec<Row>,
    plugin_keys: Vec<PluginKeyRow>,
}

impl Default for Keymap {
    /// `KEYMAP` itself: the literal table is the default keymap.
    fn default() -> Self {
        let rows = KEYMAP
            .iter()
            .filter_map(|(shown, action, context)| {
                let (key, modifiers) = parse(shown)?;
                Some(Row {
                    shown: shown.to_string(),
                    binding: Binding {
                        key,
                        modifiers,
                        context: *context,
                    },
                    action,
                    user: false,
                })
            })
            .collect();
        Keymap {
            rows,
            plugin_keys: Vec::new(),
        }
    }
}

impl Keymap {
    /// The action `key` means in `ctx`. A running turn falls back to the
    /// idle keys (`Enter` still sends, and the message queues); the last
    /// matching row wins, so a user binding beats a default.
    pub fn resolve(&self, key: KeyEvent, ctx: Context) -> Option<Action> {
        let (key, modifiers) = normalize(key.code, key.modifiers);
        let find = |context| {
            let b = Binding {
                key,
                modifiers,
                context,
            };
            self.rows.iter().rev().find(|r| r.binding == b)
        };
        let row = match (find(ctx), ctx) {
            (None, Context::Running) => find(Context::Idle),
            (row, _) => row,
        }?;
        Action::from_id(row.action)
    }

    /// `(key, action)` rows for `ctx`: `KEYMAP` order, rebound keys in
    /// place of the ones they replaced. The hints, `?` and `/help` read it.
    pub fn rows(&self, ctx: Context) -> Vec<(&str, &'static str)> {
        let rank = |a: &str| KEYMAP.iter().position(|(_, x, c)| *x == a && *c == ctx);
        let mut rows: Vec<&Row> = self
            .rows
            .iter()
            .filter(|r| r.binding.context == ctx)
            .collect();
        rows.sort_by_key(|r| rank(r.action).unwrap_or(usize::MAX));
        rows.into_iter()
            .map(|r| (r.shown.as_str(), r.action))
            .collect()
    }

    /// A user binding of `action` on `key` in each of its contexts; the key
    /// leaves any default row there, so only two user bindings conflict.
    fn put(&mut self, action: Action, key: &str) -> Result<(), String> {
        let (code, modifiers) =
            parse(key).ok_or_else(|| format!("{}: bad key {key:?}", action.id()))?;
        for context in action.contexts() {
            let binding = Binding {
                key: code,
                modifiers,
                context,
            };
            self.rows.retain(|r| r.user || r.binding != binding);
            self.rows.push(Row {
                shown: shown(key),
                binding,
                action: action.id(),
                user: true,
            });
        }
        Ok(())
    }

    /// `keybindings.toml`: `action` answers exactly `keys`. Nothing changes
    /// when one of them does not parse.
    pub fn rebind(&mut self, action: Action, keys: &[String]) -> Result<(), String> {
        if let Some(bad) = keys.iter().find(|k| parse(k).is_none()) {
            return Err(format!("{}: bad key {bad:?}", action.id()));
        }
        self.rows.retain(|r| r.action != action.id());
        keys.iter().try_for_each(|k| self.put(action, k))
    }

    /// Two actions on one key in one context, as `doctor` words them, plus
    /// two plugins whose declared keys land on the same one under the
    /// leader (T33.25, PL§8: "a clash between plugins ... is reported by
    /// `Keymap::conflicts()`"). Built-in and user bindings never clash with
    /// a plugin key: the two live in separate tables and separate lookups.
    pub fn conflicts(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (i, a) in self.rows.iter().enumerate() {
            for b in self.rows[i + 1..].iter().filter(|b| b.binding == a.binding) {
                if a.action != b.action {
                    let ctx = a.binding.context.name();
                    out.push(format!(
                        "{} in {ctx}: {} and {}",
                        b.shown, a.action, b.action
                    ));
                }
            }
        }
        for (i, a) in self.plugin_keys.iter().enumerate() {
            for b in self.plugin_keys[i + 1..]
                .iter()
                .filter(|b| b.binding == a.binding)
            {
                if a.plugin != b.plugin {
                    out.push(format!(
                        "<leader> {}: plugin {} and plugin {}",
                        a.shown, a.plugin, b.plugin
                    ));
                }
            }
        }
        out
    }

    /// `plugin`'s keys under the leader (T33.25, PL§8), replacing any it
    /// declared before — a session re-`Declare`s after every `cox_init`.
    /// A key `parse` does not recognise is dropped, same as a bad user
    /// binding. Sorted by plugin id so a clash always resolves the same
    /// way: `resolve_plugin_key` returns the first match, i.e. the lowest
    /// id (PL§8 "goes to the lower id").
    pub fn declare_plugin_keys(&mut self, plugin: &str, keys: &[KeyDecl]) {
        self.plugin_keys.retain(|r| r.plugin != plugin);
        for k in keys {
            let Some(binding) = parse(&k.key) else {
                continue;
            };
            self.plugin_keys.push(PluginKeyRow {
                shown: shown(&k.key),
                binding,
                plugin: plugin.to_string(),
                name: k.name.clone(),
            });
        }
        self.plugin_keys.sort_by(|a, b| a.plugin.cmp(&b.plugin));
    }

    /// The plugin and command name `key` means under the leader, or `None`
    /// when no plugin declared it. The lowest plugin id wins a clash
    /// (`declare_plugin_keys`'s sort order).
    pub fn resolve_plugin_key(&self, key: KeyEvent) -> Option<(&str, &str)> {
        let binding = normalize(key.code, key.modifiers);
        self.plugin_keys
            .iter()
            .find(|r| r.binding == binding)
            .map(|r| (r.plugin.as_str(), r.name.as_str()))
    }
}

/// What `load` built, and what it could not use: `warnings` are for the
/// user (their own file), `skipped` for the debug log (Claude's entries
/// cox has no action for, chords).
#[derive(Debug, Default)]
pub struct Loaded {
    pub keymap: Keymap,
    pub warnings: Vec<String>,
    pub skipped: Vec<String>,
}

/// The defaults, then Claude Code's `(key, action)` pairs added for the
/// actions both have, then `keybindings.toml` (`toml`, the file's text)
/// rebinding on top: cox's own file wins.
pub fn load(toml: Option<&str>, claude: &[(String, String)]) -> Loaded {
    let mut out = Loaded::default();
    for (key, name) in claude {
        let added = Action::from_claude(name)
            .ok_or_else(|| format!("{name} on {key}: no cox action"))
            .and_then(|action| out.keymap.put(action, key));
        if let Err(e) = added {
            out.skipped.push(format!("claude keybinding {e}"));
        }
    }
    let Some(text) = toml else { return out };
    let doc = match text.parse::<toml_edit::DocumentMut>() {
        Ok(doc) => doc,
        Err(e) => {
            out.warnings.push(format!("keybindings.toml skipped: {e}"));
            return out;
        }
    };
    let mut entries = Vec::new();
    flatten(doc.as_item(), String::new(), &mut entries);
    for (id, value) in entries {
        let keys: Option<Vec<String>> = match &value {
            toml_edit::Value::String(s) => Some(vec![s.value().clone()]),
            toml_edit::Value::Array(a) => a.iter().map(|v| v.as_str().map(String::from)).collect(),
            _ => None,
        };
        let done = match (Action::from_id(&id), keys) {
            (None, _) => Err(format!("unknown action {id:?}")),
            (Some(_), None) => Err(format!("{id}: a key or a list of keys")),
            (Some(action), Some(keys)) => out.keymap.rebind(action, &keys),
        };
        if let Err(e) = done {
            out.warnings.push(format!("keybindings.toml: {e}"));
        }
    }
    out
}

/// `mode.cycle = "shift+tab"` is a dotted key, so TOML nests it; the table
/// path joined by `.` is the action id again.
fn flatten(item: &toml_edit::Item, prefix: String, out: &mut Vec<(String, toml_edit::Value)>) {
    match item {
        toml_edit::Item::Table(table) => {
            for (key, child) in table.iter() {
                let id = if prefix.is_empty() {
                    key.to_string()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten(child, id, out);
            }
        }
        toml_edit::Item::Value(v) => out.push((prefix, v.clone())),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    /// Every `KEYMAP` row parses, and every action the enum names has a
    /// default row, so the enum and the table cannot drift apart.
    #[test]
    fn every_action_has_a_keymap_row() {
        for (k, ..) in KEYMAP {
            assert!(parse(k).is_some(), "KEYMAP key {k:?} does not parse");
        }
        for a in Action::ALL {
            assert!(!a.contexts().is_empty(), "{} has no KEYMAP row", a.id());
        }
        assert!(Keymap::default().conflicts().is_empty());
    }

    /// `docs/config.md` names every rebindable action id.
    #[test]
    fn every_action_is_documented_in_config_md() {
        let md = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/config.md"));
        let (_, section) = md
            .split_once("## `~/.cox/keybindings.toml`")
            .unwrap_or(("", ""));
        for a in Action::ALL {
            assert!(
                section.contains(&format!("`{}`", a.id())),
                "{} undocumented",
                a.id()
            );
        }
    }

    /// T25.5 step 1: modifiers in any order and case, `shift+tab` equal to
    /// the `BackTab` terminals send, `?` with or without `SHIFT`; chords
    /// and unknown names are refused.
    #[test]
    fn keymap_parses_chords() {
        let (c, a, s) = (
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::SHIFT,
        );
        assert_eq!(parse("ctrl+enter"), Some((KeyCode::Enter, c)));
        assert_eq!(parse("Alt+M"), Some((KeyCode::Char('m'), a)));
        assert_eq!(parse("shift+tab"), Some(normalize(KeyCode::BackTab, s)));
        assert_eq!(parse("?"), Some(normalize(KeyCode::Char('?'), s)));
        assert_eq!(parse("ctrl+shift+f5"), Some((KeyCode::F(5), c | s)));
        assert_eq!(parse("ctrl+x ctrl+s"), None);
        assert_eq!(parse("hyper+k"), None);
        assert_eq!(shown("ctrl+enter"), "Ctrl+Enter");
        assert_eq!(shown("alt+m"), "Alt+M");
        let map = Keymap::default();
        let backtab = key(KeyCode::BackTab, s);
        assert_eq!(map.resolve(backtab, Context::Idle), Some(Action::ModeCycle));
        assert_eq!(
            map.resolve(key(KeyCode::Tab, KeyModifiers::NONE), Context::Idle),
            None
        );
        assert_eq!(
            map.resolve(key(KeyCode::Char('?'), s), Context::Idle),
            Some(Action::Help)
        );
    }

    /// T25.5: `send = "ctrl+enter"` takes Ctrl+Enter from `newline`, leaves
    /// plain Enter unbound (the state makes it a newline), and shows in the
    /// hints; a second action on the same key is a conflict naming both.
    #[test]
    fn keymap_rebinds_send() {
        let loaded = load(
            Some("send = \"ctrl+enter\"\nmode.cycle = [\"shift+tab\"]\n"),
            &[],
        );
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        let map = &loaded.keymap;
        let ctrl_enter = key(KeyCode::Enter, KeyModifiers::CONTROL);
        assert_eq!(map.resolve(ctrl_enter, Context::Idle), Some(Action::Send));
        assert_eq!(
            map.resolve(ctrl_enter, Context::Running),
            Some(Action::SendNow)
        );
        assert_eq!(
            map.resolve(key(KeyCode::Enter, KeyModifiers::NONE), Context::Idle),
            None
        );
        let backtab = key(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(map.resolve(backtab, Context::Idle), Some(Action::ModeCycle));
        assert_eq!(
            map.rows(Context::Idle)[..2],
            [("Ctrl+Enter", "send"), ("Shift+Tab", "mode.cycle")]
        );
        assert!(map.conflicts().is_empty());

        let clash = load(
            Some("send = \"ctrl+o\"\ntranscript = \"ctrl+o\"\nwarp = \"f1\"\n"),
            &[],
        );
        assert_eq!(
            clash.keymap.conflicts(),
            ["Ctrl+O in idle: send and transcript"]
        );
        assert_eq!(
            clash.warnings,
            ["keybindings.toml: unknown action \"warp\""]
        );
    }

    /// T25.5 step 3: Claude Code's action names map onto cox's for the
    /// actions both have, added beside the defaults; everything else is
    /// skipped for the debug log, and cox's own file still wins.
    #[test]
    fn claude_keybindings_import_maps_known_commands() {
        let pairs = |v: &[(&str, &str)]| {
            v.iter()
                .map(|(k, a)| (k.to_string(), a.to_string()))
                .collect::<Vec<_>>()
        };
        let claude = pairs(&[
            ("ctrl+s", "chat:submit"),
            ("ctrl+j", "chat:newline"),
            ("alt+t", "app:toggleTranscript"),
            ("ctrl+e", "chat:externalEditor"),
            ("ctrl+x ctrl+k", "chat:cycleMode"),
        ]);
        let loaded = load(Some("newline = \"shift+enter\"\n"), &claude);
        let map = &loaded.keymap;
        let ctrl = |c| key(KeyCode::Char(c), KeyModifiers::CONTROL);
        assert_eq!(map.resolve(ctrl('s'), Context::Idle), Some(Action::Send));
        assert_eq!(
            map.resolve(key(KeyCode::Enter, KeyModifiers::NONE), Context::Idle),
            Some(Action::Send)
        );
        assert_eq!(
            map.resolve(key(KeyCode::Char('t'), KeyModifiers::ALT), Context::Running),
            Some(Action::Transcript)
        );
        assert_eq!(
            map.resolve(ctrl('j'), Context::Idle),
            None,
            "keybindings.toml replaced newline"
        );
        assert_eq!(loaded.skipped.len(), 2, "{:?}", loaded.skipped);
        assert!(loaded.warnings.is_empty());
    }

    fn decl(k: &str, name: &str) -> KeyDecl {
        KeyDecl {
            key: k.into(),
            name: name.into(),
            description: String::new(),
        }
    }

    /// T33.25, PL§8: two plugins declaring the same key under the leader
    /// both stay reachable — `resolve_plugin_key` picks the lower plugin
    /// id — but `conflicts()` still names both, the same way `doctor`
    /// already reports two built-in actions sharing a key.
    #[test]
    fn plugin_key_conflict_is_reported() {
        let mut map = Keymap::default();
        map.declare_plugin_keys("beta", &[decl("r", "run")]);
        map.declare_plugin_keys("acme", &[decl("r", "reset")]);
        assert_eq!(
            map.resolve_plugin_key(key(KeyCode::Char('r'), KeyModifiers::NONE)),
            Some(("acme", "reset")),
            "the lower plugin id wins"
        );
        assert_eq!(map.conflicts(), ["<leader> r: plugin acme and plugin beta"]);
    }

    /// A re-`Declare` (T33.44's own re-init) replaces a plugin's keys
    /// rather than piling up stale ones, and an unrecognised key is
    /// dropped instead of poisoning the table.
    #[test]
    fn declare_plugin_keys_replaces_the_previous_set() {
        let mut map = Keymap::default();
        map.declare_plugin_keys("acme", &[decl("r", "reset"), decl("chord+bad", "nope")]);
        assert_eq!(
            map.resolve_plugin_key(key(KeyCode::Char('r'), KeyModifiers::NONE)),
            Some(("acme", "reset"))
        );
        map.declare_plugin_keys("acme", &[decl("s", "save")]);
        assert_eq!(
            map.resolve_plugin_key(key(KeyCode::Char('r'), KeyModifiers::NONE)),
            None
        );
        assert_eq!(
            map.resolve_plugin_key(key(KeyCode::Char('s'), KeyModifiers::NONE)),
            Some(("acme", "save"))
        );
    }
}
