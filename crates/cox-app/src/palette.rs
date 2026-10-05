// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The command palette's ranking (DT§5.5, T37.44.13, mockup 12): one query
//! over the window's actions, the listed sessions, the session's `/`
//! commands and the project's files, grouped by kind and best first within
//! each, with the characters the query matched. Pure, so the desktop app
//! only draws what this returns and the order is tested here, not in Swift.

use cox_search::fuzzy::FuzzyQuery;
use serde::{Deserialize, Serialize};

/// What a row runs; also the order the groups show in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaletteKind {
    Action,
    Session,
    Command,
    File,
}

impl PaletteKind {
    const ALL: [Self; 4] = [Self::Action, Self::Session, Self::Command, Self::File];
}

/// One row the palette may offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaletteItem {
    pub kind: PaletteKind,
    /// An action's name, a session's id, `/name` or `@path`.
    pub id: String,
    /// What the query is matched against and the row shows.
    pub title: String,
    /// Keys, usage, or where and when a session ran.
    pub detail: String,
}

/// A row the query matched, and the offsets of its title's matched
/// characters (grapheme clusters), ascending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaletteHit {
    pub item: PaletteItem,
    pub matched: Vec<u32>,
}

/// `items` grouped by kind (actions, sessions, commands, files) and, within
/// a group, best score first, ties in the given order; at most `per_kind`
/// each. An empty query keeps every group in the given order.
pub fn rank(query: &str, items: Vec<PaletteItem>, per_kind: usize) -> Vec<PaletteHit> {
    let query = query.trim();
    let mut titles = FuzzyQuery::new(query, false);
    let mut paths = FuzzyQuery::new(query, true);
    let mut hits = Vec::new();
    for kind in PaletteKind::ALL {
        let mut group: Vec<(u32, PaletteHit)> = items
            .iter()
            .filter(|item| item.kind == kind)
            .filter_map(|item| {
                let (score, matched) = if query.is_empty() {
                    (0, Vec::new())
                } else if kind == PaletteKind::File {
                    paths.score(&item.title)?
                } else {
                    titles.score(&item.title)?
                };
                Some((
                    score,
                    PaletteHit {
                        item: item.clone(),
                        matched,
                    },
                ))
            })
            .collect();
        // Stable, so equal scores keep the caller's order (recent sessions first).
        group.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        hits.extend(group.into_iter().take(per_kind).map(|(_, hit)| hit));
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: PaletteKind, title: &str) -> PaletteItem {
        PaletteItem {
            kind,
            id: title.to_string(),
            title: title.to_string(),
            detail: String::new(),
        }
    }

    fn titles(hits: &[PaletteHit]) -> Vec<&str> {
        hits.iter().map(|h| h.item.title.as_str()).collect()
    }

    #[test]
    fn rank_groups_by_kind_and_puts_the_best_match_first_in_each() {
        let items = vec![
            item(PaletteKind::Session, "Code review for plugin loader"),
            item(PaletteKind::File, "docs/design/review.md"),
            item(PaletteKind::Action, "Show Terminal"),
            item(PaletteKind::Action, "Revert file to checkpoint…"),
            item(PaletteKind::Action, "Review changes"),
            item(PaletteKind::Command, "/code-review"),
            item(PaletteKind::Session, "Revert broken sitemap change"),
        ];
        let hits = rank("review", items, 5);
        assert_eq!(
            titles(&hits),
            vec![
                "Review changes",
                "Code review for plugin loader",
                "/code-review",
                "docs/design/review.md",
            ]
        );
        assert_eq!(hits[0].matched, vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn an_empty_query_keeps_the_given_order_with_nothing_matched() {
        let items = vec![
            item(PaletteKind::Session, "b"),
            item(PaletteKind::Action, "New session"),
            item(PaletteKind::Session, "a"),
        ];
        let hits = rank("", items, 5);
        assert_eq!(titles(&hits), vec!["New session", "b", "a"]);
        assert!(hits.iter().all(|h| h.matched.is_empty()));
    }

    #[test]
    fn each_group_is_cut_at_per_kind_and_equal_scores_keep_their_order() {
        let items = (0..4)
            .map(|n| item(PaletteKind::Session, &format!("fix {n}")))
            .collect();
        assert_eq!(titles(&rank("fix", items, 2)), vec!["fix 0", "fix 1"]);
    }

    #[test]
    fn a_query_nothing_matches_offers_no_rows() {
        let items = vec![item(PaletteKind::Action, "Review changes")];
        assert!(rank("zzqx", items, 5).is_empty());
    }
}
