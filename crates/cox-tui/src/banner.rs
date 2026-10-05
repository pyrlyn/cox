// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The pinned security banner: the one piece of TUI chrome that is not a
//! transcript cell. It lives apart from the (later) `State`/`view` so T4.3's
//! "`danger-full-access` is loud" has a home and a snapshot before the app
//! exists; `view` pins whatever `Banner::from_event` yields above the composer
//! for the rest of the session.

use cox_protocol::types::{Event, Level};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::{self, Theme};

/// Text pinned above the composer for the whole session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Banner(String);

impl Banner {
    /// A `Security` notice becomes a persistent banner; every other notice
    /// is an ordinary transcript cell and yields `None`.
    pub fn from_event(event: &Event) -> Option<Self> {
        match event {
            Event::Notice {
                level: Level::Security,
                text,
            } => Some(Self(crate::text::sanitize(text))),
            _ => None,
        }
    }

    /// One red line; the marker is styled separately so it stays visible
    /// on terminals that drop background colours.
    pub fn line(&self, theme: &Theme) -> Line<'_> {
        Line::from(vec![
            Span::styled(
                " ! ",
                Style::default()
                    .fg(theme::ALERT_FG)
                    .bg(theme.error)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {} ", self.0),
                Style::default()
                    .fg(theme.error)
                    .add_modifier(Modifier::BOLD),
            ),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;

    fn render(banner: &Banner, width: u16) -> String {
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        banner.line(&Theme::dark()).render(area, &mut buf);
        crate::view::buffer_to_string(&buf)
    }

    #[test]
    fn banner_danger_full_access() {
        let event = Event::Notice {
            level: Level::Security,
            text: cox_core::permission::policy::DANGER_FULL_ACCESS.into(),
        };
        let banner = Banner::from_event(&event).expect("security notice pins a banner");
        insta::assert_snapshot!(render(&banner, 100));
    }

    #[test]
    fn banner_ignores_non_security_notices() {
        let event = Event::Notice {
            level: Level::Warn,
            text: "hook skipped".into(),
        };
        assert_eq!(Banner::from_event(&event), None);
    }
}
