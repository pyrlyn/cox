// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The pointer rule (T37.50, A141): what the pointer looks like over an
//! element, decided once from four facts about it. Separate from every
//! client so the TUI (OSC 22), the macOS app and the Windows client map the
//! same `Pointer` to their own API and keep no rule of their own. Pure and
//! outside the `ratatui` feature, so the desktop build uses it too.

/// What a client knows about the element under the pointer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Interaction {
    /// A click does something: a button, a link, a row that opens.
    pub clickable: bool,
    /// Its text can be selected and copied.
    pub selectable: bool,
    /// Already chosen or running: the selected row, the active segment or
    /// tab, a button whose action is in flight. A click would change nothing.
    pub pressed: bool,
    /// Disabled or locked.
    pub blocked: bool,
}

/// Which way a drag handle moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// The drag moves along the vertical axis: the handle is a horizontal
    /// divider.
    Vertical,
    /// The drag moves along the horizontal axis: the handle is a vertical
    /// divider.
    Horizontal,
}

/// The pointer shape a client draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pointer {
    Default,
    Action,
    Text,
    NotAllowed,
    /// A drag handle; `pointer` never returns it, because a handle is not
    /// an `Interaction` — the client that draws one names it directly.
    Resize(Edge),
}

/// Blocked, then pressed, then clickable, then selectable: the first fact
/// that holds decides. Pressed outranks clickable so the element already
/// chosen does not invite a click, and blocked outranks both so a locked
/// control never looks live.
pub fn pointer(i: Interaction) -> Pointer {
    if i.blocked {
        Pointer::NotAllowed
    } else if i.pressed {
        Pointer::Default
    } else if i.clickable {
        Pointer::Action
    } else if i.selectable {
        Pointer::Text
    } else {
        Pointer::Default
    }
}

impl Pointer {
    /// The CSS `cursor` name, which OSC 22 takes as is (T5.9).
    pub fn css_name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Action => "pointer",
            Self::Text => "text",
            Self::NotAllowed => "not-allowed",
            Self::Resize(Edge::Vertical) => "ns-resize",
            Self::Resize(Edge::Horizontal) => "ew-resize",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: Interaction = Interaction {
        clickable: true,
        selectable: true,
        pressed: true,
        blocked: true,
    };

    #[test]
    fn blocked_wins_over_pressed_and_clickable() {
        assert_eq!(pointer(ALL), Pointer::NotAllowed);
    }

    #[test]
    fn pressed_wins_over_clickable() {
        let i = Interaction {
            blocked: false,
            ..ALL
        };
        assert_eq!(pointer(i), Pointer::Default);
    }

    #[test]
    fn clickable_wins_over_selectable() {
        let i = Interaction {
            blocked: false,
            pressed: false,
            ..ALL
        };
        assert_eq!(pointer(i), Pointer::Action);
    }

    #[test]
    fn selectable_alone_gives_text() {
        let i = Interaction {
            selectable: true,
            ..Interaction::default()
        };
        assert_eq!(pointer(i), Pointer::Text);
    }

    #[test]
    fn nothing_set_gives_default() {
        assert_eq!(pointer(Interaction::default()), Pointer::Default);
    }

    #[test]
    fn every_pointer_has_its_css_name() {
        let names = [
            (Pointer::Default, "default"),
            (Pointer::Action, "pointer"),
            (Pointer::Text, "text"),
            (Pointer::NotAllowed, "not-allowed"),
            (Pointer::Resize(Edge::Vertical), "ns-resize"),
            (Pointer::Resize(Edge::Horizontal), "ew-resize"),
        ];
        for (p, name) in names {
            assert_eq!(p.css_name(), name);
        }
    }
}
