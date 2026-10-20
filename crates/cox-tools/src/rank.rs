// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! File-graph rank for the repo map (T59.4). Separate from [`crate::repomap`]
//! so the PageRank math is a pure function of names, git order and co-change
//! pairs, with no tree and no process. The map stays a pure function of file
//! bytes, git order and the budget.

use std::collections::{HashMap, HashSet};

/// One file's names, already extracted. `defined` is the unqualified name of
/// each outline definition; `mentioned` is every identifier in the text.
pub struct FileFacts {
    pub path: String,
    pub defined: Vec<String>,
    pub mentioned: Vec<String>,
}

const DAMPING: f64 = 0.85;
const ITERATIONS: usize = 20;

/// PageRank. `edges` are `(from, to, weight)`. `personal` is a distribution
/// over the `n` nodes (it is normalised here). Damping 0.85, 20 iterations.
/// A node with no outgoing edge spreads its mass by `personal`.
pub fn pagerank(n: usize, edges: &[(usize, usize, f64)], personal: &[f64]) -> Vec<f64> {
    if n == 0 {
        return Vec::new();
    }
    let sum: f64 = personal.iter().take(n).sum();
    let personal: Vec<f64> = if sum > 0.0 {
        personal.iter().take(n).map(|p| p / sum).collect()
    } else {
        vec![1.0 / n as f64; n]
    };
    let mut out = vec![0.0; n];
    for &(from, _, weight) in edges {
        if from < n && weight > 0.0 {
            out[from] += weight;
        }
    }
    let mut rank = personal.clone();
    for _ in 0..ITERATIONS {
        let mut next = vec![0.0; n];
        let mut dangling = 0.0;
        for (node, mass) in rank.iter().enumerate() {
            if out[node] == 0.0 {
                dangling += mass;
            }
        }
        for &(from, to, weight) in edges {
            if from < n && to < n && out[from] > 0.0 && weight > 0.0 {
                next[to] += DAMPING * (weight / out[from]) * rank[from];
            }
        }
        for (node, mass) in next.iter_mut().enumerate() {
            *mass += (1.0 - DAMPING) * personal[node] + DAMPING * dangling * personal[node];
        }
        rank = next;
    }
    rank
}

/// Paths in rank order: higher PageRank first, ties in path order.
///
/// An edge runs from a file that mentions a name to the file that defines
/// it, weighted `ln(N/df)`. A name that occurs in more than 5% of files is
/// dropped. Co-change adds an edge both ways, weighted `ln(count)`, for a
/// pair that shares a commit at least twice. Personalization is seeded from
/// `recent` (newest first) with a geometric decay, plus a unit floor so a
/// file nobody has touched still has a rank.
pub fn order_files(
    files: &[FileFacts],
    recent: &[String],
    co_change: &[(String, String, u32)],
) -> Vec<String> {
    let n = files.len();
    if n == 0 {
        return Vec::new();
    }
    let index: HashMap<&str, usize> = files
        .iter()
        .enumerate()
        .map(|(i, file)| (file.path.as_str(), i))
        .collect();
    let mut defined: Vec<HashSet<&str>> = vec![HashSet::new(); n];
    let mut mentioned: Vec<HashSet<&str>> = vec![HashSet::new(); n];
    let mut df: HashMap<&str, usize> = HashMap::new();
    for (i, file) in files.iter().enumerate() {
        for name in &file.defined {
            defined[i].insert(name);
        }
        for name in &file.mentioned {
            mentioned[i].insert(name);
        }
        let mut names: HashSet<&str> = defined[i].clone();
        names.extend(mentioned[i].iter().copied());
        for name in names {
            *df.entry(name).or_insert(0) += 1;
        }
    }
    let mut weight: HashMap<(usize, usize), f64> = HashMap::new();
    for (name, count) in &df {
        if count * 20 > n {
            continue;
        }
        let idf = (n as f64 / *count as f64).ln();
        let holders: Vec<usize> = defined
            .iter()
            .enumerate()
            .filter(|(_, names)| names.contains(name))
            .map(|(i, _)| i)
            .collect();
        for (from, names) in mentioned.iter().enumerate() {
            if !names.contains(name) && !defined[from].contains(name) {
                continue;
            }
            for &to in &holders {
                if from != to {
                    *weight.entry((from, to)).or_insert(0.0) += idf;
                }
            }
        }
    }
    for (left, right, count) in co_change {
        if *count < 2 {
            continue;
        }
        let (Some(&from), Some(&to)) = (index.get(left.as_str()), index.get(right.as_str())) else {
            continue;
        };
        let w = (*count as f64).ln();
        *weight.entry((from, to)).or_insert(0.0) += w;
        *weight.entry((to, from)).or_insert(0.0) += w;
    }
    let edges: Vec<(usize, usize, f64)> = weight
        .into_iter()
        .map(|((from, to), w)| (from, to, w))
        .collect();
    let mut personal = vec![1.0; n];
    for (pos, path) in recent.iter().enumerate() {
        if let Some(&node) = index.get(path.as_str()) {
            personal[node] += 0.5_f64.powi(pos as i32) * n as f64;
        }
    }
    let rank = pagerank(n, &edges, &personal);
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        rank[b]
            .total_cmp(&rank[a])
            .then_with(|| files[a].path.cmp(&files[b].path))
    });
    order.into_iter().map(|i| files[i].path.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(got: f64, want: f64) {
        assert!((got - want).abs() < 1e-9, "{got} != {want}");
    }

    /// A 4-node cycle plus a pendant, uniform personalization, the vector
    /// from 20 iterations of the formula above.
    #[test]
    fn pagerank_on_a_4_node_graph_is_the_known_vector() {
        let edges = [(0, 1, 1.0), (1, 2, 1.0), (2, 0, 1.0), (3, 0, 1.0)];
        let rank = pagerank(4, &edges, &[0.25, 0.25, 0.25, 0.25]);
        close(rank[0], 0.3298830164);
        close(rank[1], 0.3261369643);
        close(rank[2], 0.3064800193);
        close(rank[3], 0.0375);
        close(rank.iter().sum(), 1.0);
    }

    #[test]
    fn a_name_in_more_than_five_percent_of_files_is_dropped() {
        // 40 files: `rare` is in 2 (5%, kept) and `common` is in 3 (>5%, dropped).
        let mut files = vec![
            facts("def.rs", &["rare"], &["rare"]),
            facts("use.rs", &[], &["rare"]),
            facts("hub.rs", &["common"], &["common"]),
        ];
        for name in ["m1.rs", "m2.rs"] {
            files.push(facts(name, &[], &["common"]));
        }
        for i in 0..35 {
            let path = format!("f{i}.rs");
            let own = format!("only{i}");
            files.push(facts(&path, &[&own], &[&own]));
        }
        let order = order_files(&files, &["use.rs".to_string()], &[]);
        let rare = order.iter().position(|p| p == "def.rs").expect("def");
        let hub = order.iter().position(|p| p == "hub.rs").expect("hub");
        assert!(
            rare < hub,
            "rare definer should outrank the dropped name: {order:?}"
        );
        assert!(rare < 2, "{order:?}");
    }

    /// 20 groups of four files, 200 commits that alternate a mention and the
    /// file that defines the name it uses. Top-2 hit rate against the next
    /// commit. Recency never sees that definer in the top two; the graph does.
    #[test]
    fn graph_rank_beats_recency_by_15pp_on_200_commits() {
        const GROUPS: usize = 20;
        let mut files = Vec::new();
        for group in 0..GROUPS {
            let name = format!("g{group}");
            files.push(facts(&format!("d{group}"), &[&name], &[&name]));
            for side in ["a", "b", "c"] {
                files.push(facts(&format!("{side}{group}"), &[], &[&name]));
            }
        }
        let mut commits: Vec<Vec<String>> = Vec::new();
        for step in 0..100 {
            let group = step % GROUPS;
            commits.push(vec![format!("a{group}")]);
            commits.push(vec![format!("d{group}")]);
        }
        let (graph, recent) = hit_rates(&files, &commits, 2);
        let pp = (graph - recent) * 100.0;
        // Measured on rustc 1.99 / Linux: graph 0.5025, recency 0.000, 50.3 pp
        // (100 of 199 next-commit files). The floor is the card's 15 pp.
        assert!(
            pp >= 15.0,
            "graph {graph:.3} recency {recent:.3} ({pp:.1} pp)"
        );
    }

    fn hit_rates(files: &[FileFacts], commits: &[Vec<String>], k: usize) -> (f64, f64) {
        let mut graph_hits = 0.0;
        let mut recent_hits = 0.0;
        let mut total = 0.0;
        for t in 1..commits.len() {
            let mut seen = HashSet::new();
            let mut recent = Vec::new();
            for commit in commits[..t].iter().rev() {
                for path in commit {
                    if seen.insert(path.clone()) {
                        recent.push(path.clone());
                    }
                }
            }
            let ranked = order_files(files, &recent, &[]);
            let next = &commits[t];
            total += next.len() as f64;
            for path in next {
                if ranked.iter().take(k).any(|p| p == path) {
                    graph_hits += 1.0;
                }
                if recent.iter().take(k).any(|p| p == path) {
                    recent_hits += 1.0;
                }
            }
        }
        (graph_hits / total, recent_hits / total)
    }

    fn facts(path: &str, defined: &[&str], mentioned: &[&str]) -> FileFacts {
        FileFacts {
            path: path.to_string(),
            defined: defined.iter().map(|s| (*s).to_string()).collect(),
            mentioned: mentioned.iter().map(|s| (*s).to_string()).collect(),
        }
    }
}
