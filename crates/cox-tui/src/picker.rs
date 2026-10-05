// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The picker (T5.2): one nucleo-ranked list behind `@` files, `/` commands
//! and `Ctrl+R` history. Candidates come from the caller (the workspace walk
//! is I/O and belongs to the runtime); ranking is nucleo with the same
//! path-aware config as the `glob` tool, so the user and the model find "the
//! auth handler, wherever it lives" the same way. The scorer is repeated here
//! rather than imported because plan.md §1.1 forbids `cox-tui` → `cox-tools`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32String};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;

use crate::glyph::Glyphs;
use crate::state::State;
use crate::theme::Theme;

/// Rows the list takes at most; the query narrows it, not scrolling.
const MAX_SHOWN: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Files,
    Commands,
    History,
    /// `/resume`: past sessions as `title · cwd · age · $cost` rows.
    Sessions,
    /// `Tab` on a `git` line (T15.4): a subcommand, a branch or a path.
    Shell,
    /// `/rewind` (T26.2): one row per turn, newest first.
    Rewind,
    /// After a rewind row: what to restore.
    RewindWhat,
    /// `/theme` (T24.2): built-ins, `~/.cox/themes/*.toml` stems, and every
    /// `.tmTheme` under a `syntax: ` row prefix; moving the cursor previews
    /// the row live, `Esc` reverts it.
    Themes,
    /// `/plugin new <name>` (T33.30, PL§13), when `--lang` was not given:
    /// picks the language to scaffold. Only languages PL§13 already has a
    /// template for are offered (today just Rust) — `crates/cox`'s
    /// `plugin_new::Lang` lists more (T33.34 Go, T33.36 Kotlin, T33.38
    /// Dart), each still reachable only via an explicit `--lang <name>`,
    /// which then reports its own "no template yet" notice instead of a
    /// pick that always fails.
    PluginLang,
}

/// `/plugin new`'s language picker candidates (T33.30): kept in lockstep
/// with `crates/cox`'s `plugin_new::Lang` variants that actually scaffold
/// something today — a language's template landing there (T33.34, ...)
/// adds its name here too.
pub const PLUGIN_LANGS: &[&str] = &["rust", "dart"];

/// The `RewindWhat` rows; the first word is what `Submission::Rewind` gets.
pub const REWIND_WHAT: [&str; 3] = [
    "both — files and conversation",
    "code — files only, keep the conversation",
    "talk — conversation only, keep the files",
];

/// One `/rewind` row: `T7 · 3 files · "add the cache column"`.
pub fn turn_entry(seq: u32, files: usize, text: &str) -> String {
    const MAX: usize = 48;
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut short: String = flat.chars().take(MAX).collect();
    if flat.chars().count() > MAX {
        short.push('…');
    }
    format!("T{seq} · {files} files · \"{short}\"")
}

/// What marks an agent row in the `@` picker (T45.6).
const AGENT_TAG: &str = " · agent";

/// The `@` picker's rows (T45.6): each dispatchable agent name tagged
/// `agent`, then the workspace files.
pub fn at_candidates(agents: &[String], files: &[String]) -> Vec<String> {
    agents
        .iter()
        .map(|name| format!("{name}{AGENT_TAG}"))
        .chain(files.iter().cloned())
        .collect()
}

/// A picked `@` row without its `agent` tag: what the composer gets.
pub fn untag(row: &str) -> &str {
    row.strip_suffix(AGENT_TAG).unwrap_or(row)
}

/// The turn a `turn_entry` row names.
pub fn turn_of_entry(row: &str) -> Option<u32> {
    row.strip_prefix('T')?.split(' ').next()?.parse().ok()
}

/// Git's porcelain, offered where the subcommand goes.
const GIT_COMMANDS: [&str; 28] = [
    "add",
    "bisect",
    "blame",
    "branch",
    "checkout",
    "cherry-pick",
    "clone",
    "commit",
    "diff",
    "fetch",
    "grep",
    "init",
    "log",
    "merge",
    "mv",
    "pull",
    "push",
    "rebase",
    "reset",
    "restore",
    "revert",
    "rm",
    "show",
    "stash",
    "status",
    "switch",
    "tag",
    "worktree",
];
/// Subcommands whose next word names a branch; every other takes a path.
const BRANCH_COMMANDS: [&str; 5] = ["checkout", "switch", "merge", "rebase", "branch"];

/// What `Tab` completes on a shell line: nothing unless it is a `git` line,
/// then the subcommand, a branch or a path by what stands before the word
/// being typed. Pure: the branches and files were fed into `State`.
pub fn candidates(line: &str, state: &State) -> Vec<String> {
    let Some(rest) = line.strip_prefix("git ") else {
        return Vec::new();
    };
    let mut words = rest.split_whitespace();
    let sub = words.next();
    // The subcommand counts only once the user has moved past it.
    let past_sub = words.next().is_some() || rest.ends_with(char::is_whitespace);
    match sub {
        Some(sub) if past_sub => {
            if BRANCH_COMMANDS.contains(&sub) {
                state.git_branches.clone()
            } else {
                state.files.clone()
            }
        }
        _ => GIT_COMMANDS.iter().map(|s| (*s).to_string()).collect(),
    }
}

/// The word being typed: empty after a space.
pub fn last_word(line: &str) -> &str {
    line.rsplit(char::is_whitespace).next().unwrap_or("")
}

impl Kind {
    fn prefix(self) -> &'static str {
        match self {
            Kind::Files => "@",
            Kind::Commands => "/",
            Kind::History => "history: ",
            Kind::Sessions => "resume: ",
            Kind::Shell => "complete: ",
            Kind::Rewind => "rewind to: ",
            Kind::RewindWhat => "restore: ",
            Kind::Themes => "theme: ",
            Kind::PluginLang => "lang: ",
        }
    }
}

/// How a session `depth` levels below a root is indented (T26.3): nothing
/// for a root, `└ ` under its parent, two more spaces per extra level.
/// `cox sessions` indents its id column the same way.
pub fn tree_prefix(depth: usize) -> String {
    match depth {
        0 => String::new(),
        d => format!("{}└ ", "  ".repeat(d - 1)),
    }
}

/// The `/sessions` picker header (T28.2): the project totals from the one
/// SQL aggregate in `Store::project_totals`, so the list names its cost.
pub fn project_header(slug: &str, sessions: i64, cost_usd: f64) -> String {
    format!("this project {slug} · {sessions} sessions · ${cost_usd:.2}")
}

/// One `/resume` row: title (or `untitled`), cwd, coarse age and cost;
/// a fork or handoff is indented under its parent by `depth`.
pub fn session_entry(
    depth: usize,
    title: Option<&str>,
    cwd: &str,
    age: &str,
    cost_usd: f64,
) -> String {
    format!(
        "{}{} · {} · {} · ${:.2}",
        tree_prefix(depth),
        title.unwrap_or("untitled"),
        cwd,
        age,
        cost_usd
    )
}

/// One `Ctrl+R` row from another session (T25.8): that session's coarse
/// age and the prompt's first line, cut to 80 columns.
pub fn prompt_entry(age: &str, text: &str) -> String {
    let age = match age.ends_with(['m', 'h', 'd']) {
        true => format!("{age} ago"),
        false => age.to_string(),
    };
    let first = crate::text::sanitize(text.lines().next().unwrap_or_default());
    format!("{age} · {}", crate::text::truncate(&first, 80, "..."))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picker {
    pub kind: Kind,
    pub query: String,
    all: Vec<String>,
    pub matches: Vec<String>,
    pub selected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    Nothing,
    Chosen(String),
    Closed,
}

impl Picker {
    pub fn open(kind: Kind, candidates: Vec<String>) -> Self {
        let mut picker = Self {
            kind,
            query: String::new(),
            all: candidates,
            matches: Vec::new(),
            selected: 0,
        };
        picker.refilter();
        picker
    }

    /// Starts from a query the user already typed (the last word of a
    /// shell line).
    pub fn with_query(mut self, query: &str) -> Self {
        self.query = query.to_string();
        self.refilter();
        self
    }

    fn refilter(&mut self) {
        let mut matches = self.all.clone();
        if !self.query.is_empty() {
            rank_by_query(&mut matches, &self.query);
        }
        matches.truncate(MAX_SHOWN);
        self.matches = matches;
        self.selected = 0;
    }

    pub fn key(&mut self, key: KeyEvent) -> Pick {
        match key.code {
            KeyCode::Esc => Pick::Closed,
            // Backspace past the start of the query is "never mind".
            KeyCode::Backspace => {
                if self.query.pop().is_none() {
                    return Pick::Closed;
                }
                self.refilter();
                Pick::Nothing
            }
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                Pick::Nothing
            }
            KeyCode::Down => {
                if self.selected + 1 < self.matches.len() {
                    self.selected += 1;
                }
                Pick::Nothing
            }
            KeyCode::Tab | KeyCode::Enter => match self.matches.get(self.selected) {
                Some(m) => Pick::Chosen(m.clone()),
                None => Pick::Closed,
            },
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.query.push(c);
                self.refilter();
                Pick::Nothing
            }
            _ => Pick::Nothing,
        }
    }

    pub fn height(&self) -> u16 {
        u16::try_from(1 + self.matches.len()).unwrap_or(u16::MAX)
    }

    pub fn lines(&self, g: &Glyphs, theme: &Theme) -> Vec<Line<'static>> {
        let mut lines = vec![Line::styled(
            format!(" {}{}", self.kind.prefix(), self.query),
            Style::default().add_modifier(Modifier::BOLD),
        )];
        lines.extend(self.matches.iter().enumerate().map(|(i, m)| {
            if i == self.selected {
                Line::styled(
                    format!(" {} {}", g.cursor, crate::text::sanitize(m)),
                    Style::default().fg(theme.selection),
                )
            } else {
                Line::raw(format!("   {}", crate::text::sanitize(m)))
            }
        }));
        lines
    }
}

/// Reorders `found` by nucleo's fuzzy score, best first, dropping what the
/// query does not match; ties broken by path so the order is stable.
fn rank_by_query(found: &mut Vec<String>, query: &str) {
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut scored: Vec<(u32, String)> = std::mem::take(found)
        .into_iter()
        .filter_map(|s| {
            let haystack = Utf32String::from(s.as_str());
            pattern
                .score(haystack.slice(..), &mut matcher)
                .map(|score| (score, s))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    *found = scored.into_iter().map(|(_, s)| s).collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_candidates_follow_the_word_before_the_cursor() {
        let mut state = State::new(
            cox_protocol::types::PermissionMode::Default,
            cox_protocol::types::SandboxMode::WorkspaceWrite,
        );
        state.files = vec!["src/lib.rs".into()];
        state.git_branches = vec!["main".into()];
        assert!(candidates("ls ", &state).is_empty());
        assert!(candidates("git ch", &state).contains(&"checkout".to_string()));
        assert_eq!(candidates("git ", &state).len(), GIT_COMMANDS.len());
        assert_eq!(candidates("git checkout ma", &state), ["main"]);
        assert_eq!(candidates("git switch ", &state), ["main"]);
        assert_eq!(candidates("git add sr", &state), ["src/lib.rs"]);
        assert_eq!(last_word("git add sr"), "sr");
        assert_eq!(last_word("git add "), "");
    }

    #[test]
    fn picker_turn_entry_round_trips_and_shortens_long_prompts() {
        let row = turn_entry(7, 3, "add the cache column");
        assert_eq!(row, "T7 · 3 files · \"add the cache column\"");
        assert_eq!(turn_of_entry(&row), Some(7));
        let long = turn_entry(12, 0, &"x".repeat(60));
        assert!(long.ends_with("…\""));
        assert_eq!(
            turn_entry(8, 1, "first\n\tsecond   third"),
            "T8 · 1 files · \"first second third\""
        );
        assert_eq!(turn_of_entry("both — files and conversation"), None);
    }

    #[test]
    fn picker_session_entry_lists_title_cwd_age_cost() {
        assert_eq!(
            session_entry(0, Some("auth work"), "/tmp/work", "3h", 0.83),
            "auth work · /tmp/work · 3h · $0.83"
        );
        assert_eq!(
            session_entry(0, None, "/tmp/work", "now", 0.0),
            "untitled · /tmp/work · now · $0.00"
        );
        assert_eq!(Kind::Sessions.prefix(), "resume: ");
        // The entry shape ranks under a fuzzy query like any other row.
        let picker = Picker::open(
            Kind::Sessions,
            vec![
                session_entry(0, Some("auth work"), "/tmp/work", "3h", 0.83),
                session_entry(0, Some("docs"), "/tmp/other", "9d", 0.01),
            ],
        );
        assert_eq!(picker.matches.len(), 2);
    }

    /// T28.2: the `/sessions` header names the project totals from the one
    /// SQL aggregate, so the list shows its own cost.
    #[test]
    fn picker_project_header_names_sessions_and_cost() {
        assert_eq!(
            project_header("cox", 14, 12.40),
            "this project cox · 14 sessions · $12.40"
        );
        assert_eq!(
            project_header("cox", 0, 0.0),
            "this project cox · 0 sessions · $0.00"
        );
    }

    /// T26.3's Done-when: the `/resume` picker shows a fork under its
    /// parent and a handoff of the fork one level deeper.
    #[test]
    fn picker_sessions_snapshot_nests_children() {
        let picker = Picker::open(
            Kind::Sessions,
            vec![
                session_entry(0, Some("auth work"), "/tmp/work", "3h", 0.83),
                session_entry(1, Some("auth: try b"), "/tmp/work", "2h", 0.10),
                session_entry(2, Some("auth: handoff"), "/tmp/work", "now", 0.01),
                session_entry(0, Some("docs"), "/tmp/work", "9d", 0.01),
            ],
        );
        let lines = picker.lines(&Glyphs::default(), &Theme::dark());
        let area = ratatui::layout::Rect::new(0, 0, 50, lines.len() as u16);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        ratatui::widgets::Widget::render(ratatui::widgets::Paragraph::new(lines), area, &mut buf);
        insta::assert_snapshot!(crate::view::buffer_to_string(&buf));
    }

    /// T24.2's Done-when: a picker snapshot exists for `/theme` — built-ins,
    /// a user file, and a `.tmTheme` row share one list.
    #[test]
    fn picker_themes_snapshot() {
        let picker = Picker::open(
            Kind::Themes,
            vec![
                "cox-dark".into(),
                "cox-light".into(),
                "system".into(),
                "nord".into(),
                "syntax: Solarized (dark)".into(),
            ],
        );
        let lines = picker.lines(&Glyphs::default(), &Theme::dark());
        let area = ratatui::layout::Rect::new(0, 0, 40, lines.len() as u16);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        ratatui::widgets::Widget::render(ratatui::widgets::Paragraph::new(lines), area, &mut buf);
        insta::assert_snapshot!(crate::view::buffer_to_string(&buf));
    }

    /// T45.6: the `@` picker lists dispatchable agents first, tagged
    /// `agent`, then files; a picked agent row loses its tag.
    #[test]
    fn at_picker_lists_agents_first() {
        let rows = at_candidates(
            &["explore".into(), "reviewer".into()],
            &["src/main.rs".into(), "README.md".into()],
        );
        assert_eq!(untag(&rows[1]), "reviewer");
        assert_eq!(untag(&rows[2]), "src/main.rs");
        let picker = Picker::open(Kind::Files, rows);
        let lines = picker.lines(&Glyphs::default(), &Theme::dark());
        let area = ratatui::layout::Rect::new(0, 0, 40, lines.len() as u16);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        ratatui::widgets::Widget::render(ratatui::widgets::Paragraph::new(lines), area, &mut buf);
        insta::assert_snapshot!(crate::view::buffer_to_string(&buf));
    }

    /// T33.30's Done-when: a picker snapshot exists for `/plugin new`'s
    /// language pick.
    #[test]
    fn picker_plugin_lang_snapshot() {
        let picker = Picker::open(
            Kind::PluginLang,
            PLUGIN_LANGS.iter().map(|s| (*s).to_string()).collect(),
        );
        let lines = picker.lines(&Glyphs::default(), &Theme::dark());
        let area = ratatui::layout::Rect::new(0, 0, 40, lines.len() as u16);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        ratatui::widgets::Widget::render(ratatui::widgets::Paragraph::new(lines), area, &mut buf);
        insta::assert_snapshot!(crate::view::buffer_to_string(&buf));
    }
}
