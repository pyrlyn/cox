// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Vim keys (T5.7, T25.4) over the composer's textarea, on when `tui.vim`
//! is set: motions `w b e 0 ^ $ gg G h j k l` with counts, operators
//! `d c y` over motions and text objects (`iw aw i" a" i' a' i( a( i[ a[
//! i{ a{`), `dd cc yy`, `p P x X`, `u`/`Ctrl+R`, and `v`/`V` visual modes.
//! No `.` repeat, macros or registers. `Enter` and other control keys fall
//! through so submit and interrupt still work. Separate from `composer` so
//! the non-vim path carries no modal state.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_textarea::{CursorMove, TextArea};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    Normal,
    #[default]
    Insert,
    Visual,
    VisualLine,
}

type Pos = (usize, usize);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vim {
    pub mode: Mode,
    /// The count typed so far, if any.
    count: Option<usize>,
    /// An operator waiting for its motion, with the count typed before it.
    op: Option<(char, Option<usize>)>,
    /// First key of a two-key command: `g` of `gg`, `i`/`a` of an object.
    prefix: Option<char>,
    /// Where `v`/`V` started.
    anchor: Pos,
    /// The yank came from whole lines, so `p` puts it on lines of its own.
    linewise: bool,
}

impl Vim {
    /// Runs `key` in vim terms. `true` means it was consumed here; `false`
    /// means insert mode (or a pass-through key) and the composer edits.
    pub fn key(&mut self, key: KeyEvent, area: &mut TextArea<'static>) -> bool {
        // One insert session is one undo step, as in vim.
        if !area.undo_coalescing() {
            area.set_undo_coalescing(true);
        }
        if self.mode == Mode::Insert {
            if key.code != KeyCode::Esc {
                return false;
            }
            self.mode = Mode::Normal;
            // Ends the insert run so the next insert undoes separately.
            area.set_undo_coalescing(true);
            return true;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('r') {
            for _ in 0..self.take_count() {
                area.redo();
            }
            return true;
        }
        if ctrl || key.code == KeyCode::Enter {
            return false;
        }
        match key.code {
            KeyCode::Char(c) => self.normal(c, area),
            // `Esc` (or any other key) drops what was pending and leaves
            // visual mode; it never reaches the turn (T25.4).
            _ => {
                if self.visual() {
                    area.cancel_selection();
                    self.mode = Mode::Normal;
                }
                (self.count, self.op, self.prefix) = (None, None, None);
            }
        }
        true
    }

    fn visual(&self) -> bool {
        matches!(self.mode, Mode::Visual | Mode::VisualLine)
    }

    fn take_count(&mut self) -> usize {
        self.count.take().unwrap_or(1)
    }

    fn normal(&mut self, c: char, area: &mut TextArea<'static>) {
        if self.prefix.is_none() && c.is_ascii_digit() && (c != '0' || self.count.is_some()) {
            let d = c.to_digit(10).map_or(0, |d| d as usize);
            self.count = Some(self.count.unwrap_or(0).saturating_mul(10).saturating_add(d));
            return;
        }
        let objects = self.op.is_some() || self.visual();
        if self.prefix.is_none() && (c == 'g' || (objects && matches!(c, 'i' | 'a'))) {
            self.prefix = Some(c);
            return;
        }
        let (op, before) = self.op.take().unzip();
        let count = match (self.count.take(), before.flatten()) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(1).saturating_mul(b.unwrap_or(1))),
        };
        let n = count.unwrap_or(1);
        match self.prefix.take() {
            Some('g') if c == 'g' => return self.motion_op('G', Some(n), op, area),
            Some(p @ ('i' | 'a')) => return self.object(p == 'i', c, op, area),
            Some(_) => return,
            None => {}
        }
        if let Some(op) = op {
            if c == op {
                let row = area.cursor().0;
                return self.apply(op, (row, 0), (row + n - 1, 0), true, area);
            }
            return self.motion_op(c, count, Some(op), area);
        }
        let (row, col) = area.cursor();
        let len = line_len(area, row);
        match c {
            'd' | 'c' | 'y' | 'x' if self.visual() => {
                let (a, b) = ordered(self.anchor, (row, col));
                let linewise = self.mode == Mode::VisualLine;
                area.cancel_selection();
                self.mode = Mode::Normal;
                let end = (b.0, (b.1 + 1).min(line_len(area, b.0)));
                let op = if c == 'x' { 'd' } else { c };
                self.apply(op, a, end, linewise, area);
            }
            'd' | 'c' | 'y' => self.op = Some((c, count)),
            'v' | 'V' => {
                let want = if c == 'v' {
                    Mode::Visual
                } else {
                    Mode::VisualLine
                };
                if self.mode == want {
                    area.cancel_selection();
                    self.mode = Mode::Normal;
                } else {
                    if !self.visual() {
                        self.anchor = (row, col);
                        area.start_selection();
                    }
                    self.mode = want;
                }
            }
            'i' => self.mode = Mode::Insert,
            'a' => {
                if col < len {
                    area.move_cursor(CursorMove::Forward);
                }
                self.mode = Mode::Insert;
            }
            'o' => {
                area.move_cursor(CursorMove::End);
                area.insert_newline();
                self.mode = Mode::Insert;
            }
            'x' => self.apply('d', (row, col), (row, (col + n).min(len)), false, area),
            'X' => self.apply('d', (row, col.saturating_sub(n)), (row, col), false, area),
            'p' | 'P' => self.put(c == 'P', n, area),
            'u' => {
                for _ in 0..n {
                    area.undo();
                }
            }
            _ => self.motion_op(c, count, None, area),
        }
    }

    /// Moves by motion `c`; with `op`, applies it over the span covered.
    fn motion_op(
        &mut self,
        c: char,
        count: Option<usize>,
        op: Option<char>,
        area: &mut TextArea<'static>,
    ) {
        let from = area.cursor();
        let on_word = area
            .lines()
            .get(from.0)
            .and_then(|l| l.chars().nth(from.1))
            .is_some_and(|ch| !ch.is_whitespace());
        // `cw` on a word changes to its end, like `ce`.
        let c = if c == 'w' && op == Some('c') && on_word {
            'e'
        } else {
            c
        };
        let Some((linewise, inclusive)) = motion(c, count, area) else {
            return;
        };
        let Some(op) = op else { return };
        let mut to = area.cursor();
        if c == 'w' && to.0 > from.0 {
            // `dw` on a line's last word stops at the line's end.
            to = (from.0, line_len(area, from.0));
        }
        if inclusive {
            to.1 = (to.1 + 1).min(line_len(area, to.0));
        }
        self.apply(op, from, to, linewise, area);
    }

    fn object(&mut self, inner: bool, c: char, op: Option<char>, area: &mut TextArea<'static>) {
        let Some((from, to)) = object(area.lines(), area.cursor(), inner, c) else {
            return;
        };
        match op {
            Some(op) => self.apply(op, from, to, false, area),
            None => {
                self.anchor = from;
                area.cancel_selection();
                jump(area, from);
                area.start_selection();
                let last = if to.1 > 0 {
                    (to.0, to.1 - 1)
                } else {
                    let row = to.0.saturating_sub(1);
                    (row, line_len(area, row))
                };
                jump(area, last);
            }
        }
    }

    /// `d`, `c` or `y` over `from..to`; whole rows when `linewise`. One
    /// `cut` per operator, so one `u` undoes it.
    fn apply(
        &mut self,
        op: char,
        from: Pos,
        to: Pos,
        linewise: bool,
        area: &mut TextArea<'static>,
    ) {
        let (from, to) = ordered(from, to);
        self.linewise = linewise;
        if !linewise {
            select(area, from, to);
            if op == 'y' {
                area.copy();
                area.cancel_selection();
                jump(area, from);
            } else {
                area.cut();
            }
        } else {
            let last = area.lines().len().saturating_sub(1);
            let (r0, r1) = (from.0, to.0.min(last));
            let text = area.lines().get(r0..=r1).unwrap_or_default().join("\n");
            let end = (r1, line_len(area, r1));
            match op {
                'y' => jump(area, (r0, from.1)),
                'c' => {
                    select(area, (r0, 0), end);
                    area.cut();
                }
                _ => {
                    if r1 < last {
                        select(area, (r0, 0), (r1 + 1, 0));
                    } else if r0 > 0 {
                        select(area, (r0 - 1, line_len(area, r0 - 1)), end);
                    } else {
                        select(area, (0, 0), end);
                    }
                    area.cut();
                    let row = r0.min(area.lines().len().saturating_sub(1));
                    jump(area, (row, first_non_blank(area, row)));
                }
            }
            area.set_yank_text(text);
        }
        if op == 'c' {
            self.mode = Mode::Insert;
        }
    }

    /// `p`/`P`: whole lines below/above after a linewise yank, otherwise
    /// the text after/at the cursor.
    fn put(&self, before: bool, n: usize, area: &mut TextArea<'static>) {
        let text = area.yank_text();
        let (row, col) = area.cursor();
        if self.linewise {
            let block = vec![text; n].join("\n");
            if before {
                area.move_cursor(CursorMove::Head);
                area.insert_str(format!("{block}\n"));
                jump(area, (row, 0));
            } else {
                area.move_cursor(CursorMove::End);
                area.insert_str(format!("\n{block}"));
                jump(area, (row + 1, 0));
            }
        } else {
            if !before && col < line_len(area, row) {
                area.move_cursor(CursorMove::Forward);
            }
            area.insert_str(text.repeat(n));
        }
    }
}

/// Moves the cursor by motion `c`; `(linewise, inclusive)` says how an
/// operator treats the span, `None` when `c` is not a motion.
fn motion(c: char, count: Option<usize>, area: &mut TextArea<'static>) -> Option<(bool, bool)> {
    let n = count.unwrap_or(1);
    let (row, col) = area.cursor();
    let repeat = |area: &mut TextArea<'static>, mv: CursorMove, times: usize| {
        for _ in 0..times {
            area.move_cursor(mv);
        }
    };
    match c {
        'h' => jump(area, (row, col.saturating_sub(n))),
        'l' => jump(area, (row, (col + n).min(line_len(area, row)))),
        'j' => repeat(area, CursorMove::Down, n),
        'k' => repeat(area, CursorMove::Up, n),
        'w' => repeat(area, CursorMove::WordForward, n),
        'b' => repeat(area, CursorMove::WordBack, n),
        'e' => repeat(area, CursorMove::WordEnd, n),
        '0' => area.move_cursor(CursorMove::Head),
        '^' => jump(area, (row, first_non_blank(area, row))),
        '$' => {
            repeat(area, CursorMove::Down, n - 1);
            area.move_cursor(CursorMove::End);
        }
        // `gg` arrives here as `G` with a count of 1.
        'G' => {
            let last = area.lines().len().saturating_sub(1);
            let to = count.map_or(last, |g| g.saturating_sub(1).min(last));
            jump(area, (to, first_non_blank(area, to)));
        }
        _ => return None,
    }
    Some((matches!(c, 'j' | 'k' | 'G'), matches!(c, 'e' | '$')))
}

/// The span of text object `c` around `at`, end exclusive. Computed over
/// the flattened text so `i(` and friends may span lines.
fn object(lines: &[String], at: Pos, inner: bool, c: char) -> Option<(Pos, Pos)> {
    let text: Vec<char> = lines.join("\n").chars().collect();
    let flat = lines[..at.0.min(lines.len())]
        .iter()
        .map(|l| l.chars().count() + 1)
        .sum::<usize>()
        + at.1;
    let (s, e) = match c {
        'w' => word(&text, flat, inner)?,
        '"' | '\'' | '`' => quote(&text, flat, c, inner)?,
        '(' | ')' | 'b' => pair(&text, flat, ('(', ')'), inner)?,
        '[' | ']' => pair(&text, flat, ('[', ']'), inner)?,
        '{' | '}' | 'B' => pair(&text, flat, ('{', '}'), inner)?,
        _ => return None,
    };
    Some((unflatten(lines, s), unflatten(lines, e)))
}

fn unflatten(lines: &[String], mut i: usize) -> Pos {
    for (row, line) in lines.iter().enumerate() {
        let len = line.chars().count();
        if i <= len {
            return (row, i);
        }
        i -= len + 1;
    }
    let row = lines.len().saturating_sub(1);
    (row, lines.get(row).map_or(0, |l| l.chars().count()))
}

/// Whitespace, word characters, punctuation: `iw` spans one class.
fn class(c: char) -> u8 {
    match c {
        c if c.is_whitespace() => 0,
        c if c.is_alphanumeric() || c == '_' => 1,
        _ => 2,
    }
}

fn word(t: &[char], at: usize, inner: bool) -> Option<(usize, usize)> {
    let k = class(*t.get(at).filter(|c| **c != '\n')?);
    let same = |i: usize, k: u8| t.get(i).is_some_and(|c| *c != '\n' && class(*c) == k);
    let (mut s, mut e) = (at, at);
    while s > 0 && same(s - 1, k) {
        s -= 1;
    }
    while same(e, k) {
        e += 1;
    }
    if !inner {
        // `aw` takes the whitespace after the word, or before it when there
        // is none; on whitespace it takes the word that follows.
        let next = t.get(e).map_or(k, |c| class(*c));
        if k == 0 {
            while same(e, next) {
                e += 1;
            }
        } else if same(e, 0) {
            while same(e, 0) {
                e += 1;
            }
        } else {
            while s > 0 && same(s - 1, 0) {
                s -= 1;
            }
        }
    }
    Some((s, e))
}

/// `i"`/`a"`: the quoted span on the cursor's line that contains or
/// follows the cursor; quotes pair up left to right, as in vim.
fn quote(t: &[char], at: usize, q: char, inner: bool) -> Option<(usize, usize)> {
    let start = t[..at.min(t.len())]
        .iter()
        .rposition(|c| *c == '\n')
        .map_or(0, |i| i + 1);
    let end = t[at.min(t.len())..]
        .iter()
        .position(|c| *c == '\n')
        .map_or(t.len(), |i| at + i);
    let quotes: Vec<usize> = (start..end).filter(|&i| t[i] == q).collect();
    let (o, c) = quotes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&[o, c]| (o, c))
        .find(|&(_, c)| at <= c)?;
    Some(if inner { (o + 1, c) } else { (o, c + 1) })
}

/// `i(`/`a(` and friends: the innermost balanced pair around `at`.
fn pair(t: &[char], at: usize, (open, close): (char, char), inner: bool) -> Option<(usize, usize)> {
    let last = t.len().checked_sub(1)?;
    let mut depth = 0usize;
    let mut o = None;
    for i in (0..=at.min(last)).rev() {
        if t[i] == close && i != at {
            depth += 1;
        } else if t[i] == open {
            if depth == 0 {
                o = Some(i);
                break;
            }
            depth -= 1;
        }
    }
    let o = o?;
    let mut depth = 0usize;
    let c = (o + 1..t.len()).find(|&i| {
        if t[i] == open {
            depth += 1;
        } else if t[i] == close {
            if depth == 0 {
                return true;
            }
            depth -= 1;
        }
        false
    })?;
    Some(if inner { (o + 1, c) } else { (o, c + 1) })
}

fn ordered(a: Pos, b: Pos) -> (Pos, Pos) {
    if a <= b { (a, b) } else { (b, a) }
}

fn line_len(area: &TextArea<'static>, row: usize) -> usize {
    area.lines().get(row).map_or(0, |l| l.chars().count())
}

fn first_non_blank(area: &TextArea<'static>, row: usize) -> usize {
    area.lines()
        .get(row)
        .map_or(0, |l| l.chars().take_while(|c| c.is_whitespace()).count())
}

/// Moves to `pos`; extends the selection when one is active.
fn jump(area: &mut TextArea<'static>, (row, col): Pos) {
    let clamp = |v: usize| u16::try_from(v).unwrap_or(u16::MAX);
    area.move_cursor(CursorMove::Jump(clamp(row), clamp(col)));
}

fn select(area: &mut TextArea<'static>, from: Pos, to: Pos) {
    area.cancel_selection();
    jump(area, from);
    area.start_selection();
    jump(area, to);
}
