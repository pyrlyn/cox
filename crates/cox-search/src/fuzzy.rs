// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! One query scored against many short texts with `nucleo`, returning which
//! characters matched as well as the score: what a picker needs to draw the
//! matched characters bold (the desktop command palette, T37.44.13). Separate
//! from `glob::rank_by_query`, which only orders paths and never shows why.

use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32String};

/// A parsed query and the matcher state it reuses across texts.
pub struct FuzzyQuery {
    pattern: Pattern,
    matcher: Matcher,
    indices: Vec<u32>,
}

impl FuzzyQuery {
    /// `paths` scores `/` as a word boundary and favours the file name, as
    /// `glob`'s ranking does; titles and command names leave it off.
    pub fn new(query: &str, paths: bool) -> Self {
        let config = if paths {
            Config::DEFAULT.match_paths()
        } else {
            Config::DEFAULT
        };
        Self {
            pattern: Pattern::parse(query, CaseMatching::Smart, Normalization::Smart),
            matcher: Matcher::new(config),
            indices: Vec::new(),
        }
    }

    /// `text`'s score and the offsets of its matched characters, ascending
    /// and each once; `None` when the query does not match. An offset counts
    /// grapheme clusters, as `Utf32String` splits the text, which is what a
    /// Swift `Character` is.
    pub fn score(&mut self, text: &str) -> Option<(u32, Vec<u32>)> {
        let haystack = Utf32String::from(text);
        self.indices.clear();
        let score =
            self.pattern
                .indices(haystack.slice(..), &mut self.matcher, &mut self.indices)?;
        // Several atoms each push their own offsets, unsorted and overlapping.
        let mut matched = std::mem::take(&mut self.indices);
        matched.sort_unstable();
        matched.dedup();
        Some((score, matched))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_reports_the_matched_characters_in_order_once() {
        let mut query = FuzzyQuery::new("rev re", false);
        let (_, matched) = query.score("Review changes").expect("matches");
        assert_eq!(matched, vec![0, 1, 2]);
    }

    #[test]
    fn a_query_that_does_not_match_scores_nothing() {
        assert!(
            FuzzyQuery::new("zzqx", false)
                .score("Review changes")
                .is_none()
        );
    }

    #[test]
    fn offsets_count_characters_not_bytes() {
        let mut query = FuzzyQuery::new("rev", false);
        let (_, matched) = query.score("é review").expect("matches");
        assert_eq!(matched, vec![2, 3, 4]);
    }
}
