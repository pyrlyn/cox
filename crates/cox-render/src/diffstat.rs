// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The `+n −m` line counts of a unified diff. Outside `diff` because that
//! module draws with ratatui, and the desktop app (`cox-app`, built without
//! the `ratatui` feature) writes the same counts into its tool summaries.

/// Added and removed line counts of a unified diff; file markers do not count.
pub fn counts(unified: &str) -> (usize, usize) {
    unified.lines().fold((0, 0), |(a, r), l| {
        if l.starts_with("+++") || l.starts_with("---") {
            (a, r)
        } else if l.starts_with('+') {
            (a + 1, r)
        } else if l.starts_with('-') {
            (a, r + 1)
        } else {
            (a, r)
        }
    })
}
