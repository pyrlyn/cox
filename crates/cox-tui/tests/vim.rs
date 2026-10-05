// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Vim keys (T5.7, T25.4): keypress tables over the composer with `tui.vim`
//! on, and the `Esc`/status-line behaviour through `update`.

use cox_protocol::Submission;
use cox_protocol::types::{PermissionMode, SandboxMode};
use cox_tui::composer::Composer;
use cox_tui::state::{Cmd, Msg, State, update};
use cox_tui::vim::Mode;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rstest::rstest;

/// `⎋` is `Esc`, `↻` is `Ctrl+R`; everything else is typed as is.
fn press(c: &mut Composer, keys: &str) {
    for ch in keys.chars() {
        let key = match ch {
            '⎋' => KeyEvent::from(KeyCode::Esc),
            '↻' => KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            _ => KeyEvent::from(KeyCode::Char(ch)),
        };
        // No turn runs in these tables; `busy` never changes the outcome.
        c.key(key, false);
    }
}

#[test]
fn vim_keypress_table() {
    let mut c = Composer::new();
    c.set_vim(true);
    c.set_text("one two three\nfour five");
    // Cursor starts at the end of the inserted text.
    let table: &[(&str, &str, (usize, usize), Mode)] = &[
        ("⎋", "one two three\nfour five", (1, 9), Mode::Normal),
        ("k0", "one two three\nfour five", (0, 0), Mode::Normal),
        ("w", "one two three\nfour five", (0, 4), Mode::Normal),
        ("3l", "one two three\nfour five", (0, 7), Mode::Normal),
        ("$", "one two three\nfour five", (0, 13), Mode::Normal),
        ("b", "one two three\nfour five", (0, 8), Mode::Normal),
        ("j", "one two three\nfour five", (1, 8), Mode::Normal),
        ("0x", "one two three\nour five", (1, 0), Mode::Normal),
        ("2x", "one two three\nr five", (1, 0), Mode::Normal),
        ("kdd", "r five", (0, 0), Mode::Normal),
        ("yyp", "r five\nr five", (1, 0), Mode::Normal),
        ("i", "r five\nr five", (1, 0), Mode::Insert),
        ("X⎋", "r five\nXr five", (1, 1), Mode::Normal),
        ("a", "r five\nXr five", (1, 2), Mode::Insert),
        ("⎋o", "r five\nXr five\n", (2, 0), Mode::Insert),
        ("new⎋", "r five\nXr five\nnew", (2, 3), Mode::Normal),
        // `dd` on the last line takes the newline before it too.
        ("dd", "r five\nXr five", (1, 0), Mode::Normal),
    ];
    for (keys, text, cursor, mode) in table {
        press(&mut c, keys);
        assert_eq!(c.text(), *text, "after {keys:?}");
        assert_eq!(c.cursor(), *cursor, "after {keys:?}");
        assert_eq!(c.vim_mode(), Some(*mode), "after {keys:?}");
    }
}

/// Each row starts from `start` in insert mode with the cursor at its end,
/// presses `Esc`, then `keys`.
#[rstest]
// Motions with counts.
#[case::w_b_e_with_counts("one two three four", "02we2b", "one two three four", (0, 4), Mode::Normal)]
#[case::caret_and_zero("   indented", "0^", "   indented", (0, 3), Mode::Normal)]
#[case::gg_and_count_g("a\nb\nc", "gg2G", "a\nb\nc", (1, 0), Mode::Normal)]
#[case::g_to_last_line("a\nb\nc", "ggG", "a\nb\nc", (2, 0), Mode::Normal)]
#[case::hjkl_with_counts("abcdef\nabcdef\nabcdef", "2k3hj2l", "abcdef\nabcdef\nabcdef", (1, 5), Mode::Normal)]
// Operators over motions, doubled operators, put, x/X, undo/redo.
#[case::d_count_w("one two three", "0d2w", "three", (0, 0), Mode::Normal)]
#[case::dw_stops_at_line_end("one two\nthree", "ggwdw", "one \nthree", (0, 4), Mode::Normal)]
#[case::de_is_inclusive("one two three", "0de", " two three", (0, 0), Mode::Normal)]
#[case::d_dollar("one two three", "0wd$", "one ", (0, 4), Mode::Normal)]
#[case::cw_changes_to_word_end("one two three", "0cwONE⎋", "ONE two three", (0, 3), Mode::Normal)]
#[case::cc_changes_the_line("one\ntwo\nthree", "ggjccX⎋", "one\nX\nthree", (1, 1), Mode::Normal)]
#[case::dj_is_linewise("a\nb\nc", "ggdj", "c", (0, 0), Mode::Normal)]
#[case::dgg_is_linewise("a\nb\nc", "kdgg", "c", (0, 0), Mode::Normal)]
#[case::yy_big_p_puts_above("one\ntwo", "ggyyjP", "one\none\ntwo", (1, 0), Mode::Normal)]
#[case::yw_big_p_is_charwise("one two", "0ywP", "one one two", (0, 4), Mode::Normal)]
#[case::x_and_big_x_with_counts("abcdef", "03x$X", "de", (0, 2), Mode::Normal)]
// The cursor lands where `tui-textarea`'s history puts it, not on the
// edit's start as in vim.
#[case::u_undoes_an_operator("one two three", "0dwdwu", "two three", (0, 4), Mode::Normal)]
#[case::u_undoes_an_insert_run("one", "aXYZ⎋u", "one", (0, 3), Mode::Normal)]
#[case::ctrl_r_redoes("one two three", "0dwu↻", "two three", (0, 0), Mode::Normal)]
// Text objects.
#[case::diw("one two three", "0wdiw", "one  three", (0, 4), Mode::Normal)]
#[case::daw("one two three", "0wdaw", "one three", (0, 4), Mode::Normal)]
#[case::ci_double_quote("say \"hi there\" now", "0ci\"bye⎋", "say \"bye\" now", (0, 8), Mode::Normal)]
#[case::da_single_quote("x 'a b' y", "0da'", "x  y", (0, 2), Mode::Normal)]
#[case::di_paren_nested("f(a, (b), c)", "0wdi(", "f()", (0, 2), Mode::Normal)]
#[case::ca_paren("f(a) + g", "0wca(X⎋", "fX + g", (0, 2), Mode::Normal)]
#[case::da_bracket("x [1, [2]] y", "0wda[", "x  y", (0, 2), Mode::Normal)]
#[case::ci_bracket("[a, b]", "0ci[z⎋", "[z]", (0, 2), Mode::Normal)]
#[case::di_brace_spans_lines("f {\n  body\n}", "ggjdi{", "f {}", (0, 3), Mode::Normal)]
#[case::da_brace("{x} y", "0da{", " y", (0, 0), Mode::Normal)]
#[case::di_single_quote("'abc'", "0di'", "''", (0, 1), Mode::Normal)]
#[case::da_double_quote("a \"b\" c", "0da\"", "a  c", (0, 2), Mode::Normal)]
// Visual modes.
#[case::v_d_is_inclusive("one two three", "0vlld", " two three", (0, 0), Mode::Normal)]
#[case::v_y_then_put("one two three", "0wvey0P", "twoone two three", (0, 3), Mode::Normal)]
#[case::v_c_inserts("one two three", "0vecX⎋", "X two three", (0, 1), Mode::Normal)]
#[case::v_text_object("one two three", "0wviwd", "one  three", (0, 4), Mode::Normal)]
#[case::big_v_d_takes_lines("a\nb\nc", "ggVjd", "c", (0, 0), Mode::Normal)]
#[case::big_v_y_then_p("a\nb", "ggVyjp", "a\nb\na", (2, 0), Mode::Normal)]
#[case::v_mode_shows("abc", "0vl", "abc", (0, 1), Mode::Visual)]
#[case::big_v_mode_shows("abc", "0V", "abc", (0, 0), Mode::VisualLine)]
#[case::esc_leaves_visual("abc", "0vl⎋", "abc", (0, 1), Mode::Normal)]
// `Esc` in normal mode changes nothing and cancels a pending operator.
#[case::esc_in_normal_is_a_no_op("abc", "0⎋⎋d⎋x", "bc", (0, 0), Mode::Normal)]
fn vim_key_table(
    #[case] start: &str,
    #[case] keys: &str,
    #[case] text: &str,
    #[case] cursor: (usize, usize),
    #[case] mode: Mode,
) {
    let mut c = Composer::new();
    c.set_vim(true);
    c.set_text(start);
    press(&mut c, &format!("⎋{keys}"));
    assert_eq!(c.text(), text, "text after {keys:?}");
    assert_eq!(c.cursor(), cursor, "cursor after {keys:?}");
    assert_eq!(c.vim_mode(), Some(mode), "mode after {keys:?}");
}

#[test]
fn vim_off_leaves_keys_alone_and_slash_vim_toggles_it() {
    let mut c = Composer::new();
    press(&mut c, "hjkl");
    assert_eq!(c.text(), "hjkl");
    assert_eq!(c.vim_mode(), None);

    let mut state = State::new(PermissionMode::Default, SandboxMode::ReadOnly);
    for k in [KeyCode::Char('/'), KeyCode::Esc] {
        update(&mut state, Msg::Key(KeyEvent::from(k)));
    }
    for ch in "vim".chars() {
        update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char(ch))));
    }
    assert!(update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Enter))).is_empty());
    assert_eq!(state.composer.vim_mode(), Some(Mode::Insert));
    let line = cox_tui::status::line(&state).to_string();
    assert!(line.ends_with("-- INSERT --"), "{line}");
    update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(
        cox_tui::status::line(&state)
            .to_string()
            .ends_with("-- NORMAL --")
    );
    update(&mut state, Msg::Key(KeyEvent::from(KeyCode::Char('v'))));
    assert!(
        cox_tui::status::line(&state)
            .to_string()
            .ends_with("-- VISUAL --")
    );
}

#[test]
fn esc_in_normal_mode_never_interrupts_the_turn() {
    let mut state = State::new(PermissionMode::Default, SandboxMode::ReadOnly);
    state.composer.set_vim(true);
    state.composer.set_text("draft");
    let esc = || Msg::Key(KeyEvent::from(KeyCode::Esc));
    state.status.busy = true;
    // Insert mode: `Esc` interrupts, as it always has.
    assert_eq!(
        update(&mut state, esc()),
        vec![Cmd::Submit(Submission::Interrupt)]
    );
    state.status.busy = false;
    update(&mut state, esc());
    assert_eq!(state.composer.vim_mode(), Some(Mode::Normal));
    state.status.busy = true;
    assert!(update(&mut state, esc()).is_empty());
    assert_eq!(state.composer.text(), "draft");
    assert_eq!(state.composer.vim_mode(), Some(Mode::Normal));
}
