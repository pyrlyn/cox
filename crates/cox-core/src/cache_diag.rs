// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Cache diagnostics (T8.3 §1.9/D6): is the prefix actually hitting the
//! cache, and if not, which block broke it? Pure functions over a request
//! copy plus a tiny per-session tracker; the session emits the `Notice`,
//! the TUI shows the ratio, `cox stats --cache` lists the misses.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use cox_protocol::types::Usage;

use crate::context::repomap_bytes;

/// Share of this call served from cache: `read / (input + read + write)`.
pub fn ratio_of(usage: &Usage) -> f64 {
    ratio(
        usage.input_tokens,
        usage.cache_read_tokens,
        usage.cache_write_tokens,
    )
}

/// Same, over raw counters; 0 when there is nothing to share.
pub fn ratio(input: u32, read: u32, write: u32) -> f64 {
    let denom = u64::from(input) + u64::from(read) + u64::from(write);
    if denom == 0 {
        0.0
    } else {
        f64::from(read) / denom as f64
    }
}

/// Status-line form: `cache 87%`.
pub fn format_ratio(ratio: f64) -> String {
    format!("cache {}%", (ratio * 100.0).round() as u64)
}

/// Human name for a prefix block index: `system[0..=2]` then messages.
pub fn block_name(index: usize, system_len: usize) -> String {
    match index {
        0 => "system[0] tools".into(),
        1 => "system[1] system prompt".into(),
        2 => "system[2] instruction files".into(),
        i if i < system_len => format!("system[{i}] volatile"),
        i => format!("message {}", i - system_len),
    }
}

/// One block's bytes, hashed; the session keeps one per prefix block.
pub fn hash_block(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// First byte where two blocks differ, for the miss message.
pub fn first_byte_diff(a: &str, b: &str) -> usize {
    a.bytes()
        .zip(b.bytes())
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()))
}

/// Remembers the last request's prefix bytes and whether it hit the cache.
/// `observe` returns the miss `Notice` text when a call that used to hit
/// now reads nothing — the byte that broke it, by block name.
#[derive(Debug, Default)]
pub struct CacheTracker {
    prev_texts: Vec<String>,
    prev_hashes: Vec<u64>,
    had_cache: bool,
}

impl CacheTracker {
    /// Empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Last call's cache share (0 before the first call).
    pub fn observe(&mut self, system_texts: &[String], usage: &Usage) -> Option<String> {
        let hashes: Vec<u64> = system_texts.iter().map(|t| hash_block(t)).collect();
        let miss = self.had_cache && usage.cache_read_tokens == 0 && !self.prev_hashes.is_empty();
        let notice = miss
            .then(|| self.miss_text(system_texts, &hashes))
            .flatten();
        self.prev_texts = system_texts.to_vec();
        self.prev_hashes = hashes;
        self.had_cache = usage.cache_read_tokens > 0;
        notice
    }

    fn miss_text(&self, texts: &[String], hashes: &[u64]) -> Option<String> {
        let system_len = texts.len();
        for (i, (old_h, new_h)) in self.prev_hashes.iter().zip(hashes.iter()).enumerate() {
            if old_h != new_h {
                let old = self.prev_texts.get(i).map(String::as_str).unwrap_or("");
                let new = texts.get(i).map(String::as_str).unwrap_or("");
                // P43: a `/repomap refresh` changes the end of system[2],
                // so a miss there names the map as a suspect.
                let with_map = i == 2 && (repomap_bytes(old) > 0 || repomap_bytes(new) > 0);
                let name = if with_map {
                    "system[2] instructions + repo map".to_string()
                } else {
                    block_name(i, system_len)
                };
                return Some(format!(
                    "cache miss: {name} changed at byte {}",
                    first_byte_diff(old, new)
                ));
            }
        }
        if hashes.len() != self.prev_hashes.len() {
            return Some(format!(
                "cache miss: prefix length {} → {}",
                self.prev_hashes.len(),
                hashes.len()
            ));
        }
        Some("cache miss: prefix bytes identical but cache read is 0".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u32, read: u32, write: u32) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: 0,
            cache_read_tokens: read,
            cache_write_tokens: write,
            estimated: false,
            cost_usd: 0.0,
            latency_ms: 0,
        }
    }

    #[test]
    fn cache_diag_ratio_is_read_over_total() {
        assert_eq!(ratio(0, 0, 0), 0.0);
        assert!((ratio(10, 87, 3) - 0.87).abs() < 1e-9);
        assert_eq!(format_ratio(0.87), "cache 87%");
        assert!((ratio_of(&usage(10, 80, 10)) - 0.8).abs() < 1e-9);
    }

    #[test]
    fn cache_diag_miss_names_the_changed_block() {
        let mut t = CacheTracker::new();
        let sys = |tools: &str| {
            vec![
                tools.into(),
                "prompt".into(),
                "instructions".into(),
                "volatile".into(),
            ]
        };
        assert_eq!(t.observe(&sys("tools"), &usage(10, 90, 0)), None);
        let notice = t.observe(&sys("tools"), &usage(100, 0, 0)).expect("miss");
        // Nothing changed: identical-bytes fallback names no block.
        assert!(notice.contains("cache miss"), "{notice}");
        let notice = {
            let mut t2 = CacheTracker::new();
            t2.observe(&sys("tools"), &usage(10, 90, 0));
            t2.observe(&sys("tools+v2"), &usage(100, 0, 0))
                .expect("miss")
        };
        assert!(notice.contains("system[0] tools"), "{notice}");
    }

    #[test]
    fn cache_diag_volatile_byte_is_flagged_with_block_name() {
        let mut t = CacheTracker::new();
        let base = vec!["t".into(), "p".into(), "i".into(), "date=mon".into()];
        assert_eq!(t.observe(&base, &usage(5, 95, 0)), None);
        let changed = vec!["t".into(), "p".into(), "i".into(), "date=tue".into()];
        let notice = t.observe(&changed, &usage(100, 0, 0)).expect("miss");
        assert!(notice.contains("system[3] volatile"), "{notice}");
        assert!(notice.contains("byte 5"), "{notice}");
    }

    /// P43: a refreshed map breaks the prefix at the end of system[2]; the
    /// miss names the map so it is not blamed on the instruction files.
    #[test]
    fn cache_diag_names_the_repo_map_in_system_two() {
        let mut t = CacheTracker::new();
        let sys = |map: &str| {
            vec![
                "t".into(),
                "p".into(),
                format!("i\n<repo_map>\n{map}\n</repo_map>"),
                "v".into(),
            ]
        };
        assert_eq!(t.observe(&sys("a.rs"), &usage(5, 95, 0)), None);
        let notice = t.observe(&sys("b.rs"), &usage(100, 0, 0)).expect("miss");
        assert!(
            notice.contains("system[2] instructions + repo map"),
            "{notice}"
        );
        let mut plain = CacheTracker::new();
        let base = vec!["t".into(), "p".into(), "i".into(), "v".into()];
        plain.observe(&base, &usage(5, 95, 0));
        let changed = vec!["t".into(), "p".into(), "j".into(), "v".into()];
        let notice = plain.observe(&changed, &usage(100, 0, 0)).expect("miss");
        assert!(notice.contains("system[2] instruction files"), "{notice}");
    }
}
