// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! End-of-session memory extraction (T10.2): with `memory.extract`, a
//! `Shutdown` runs one cheap `memory`-job call over the transcript, asks the
//! `salience` decision point (T33.21.1, `monotone.rs`) to drop candidates
//! scoring below `[memory].salience_min`, dedups what is left against the
//! store's FTS rows (trigram similarity over 0.8 means "already known") and
//! upserts the survivors. Failures warn, never fail the shutdown; the
//! `SessionEnd` hook fires after, either way.
//!
//! The survivors land in the store (searchable at once) and in
//! `drain_extracted` for surfaces, which own the `.md` files the core must
//! never touch directly.

use std::collections::HashSet;

use cox_protocol::errors::CoreError;
use cox_protocol::types::{
    Content, Event, Job, Level, Message, ProviderEvent, Request, Role, SystemBlock,
};
use tokio::sync::mpsc;

use crate::budget;
use crate::compact;
use crate::session::Session;

const PROMPT: &str = include_str!("prompts/memory.md");
/// The extraction answer is short by construction.
const MAX_FACT_TOKENS: u32 = 2048;
/// A candidate more similar than this to a stored fact is already known.
const DEDUP_SIMILARITY: f64 = 0.8;

/// One extracted fact, ready to save.
#[derive(Debug, Clone, PartialEq)]
pub struct Fact {
    /// Fact slug (`[a-z0-9-]`).
    pub name: String,
    /// `decision`, `fact`, `gotcha`, default `fact`.
    pub kind: String,
    /// Self-contained body.
    pub body: String,
}

/// Parses the extraction answer: a JSON array of `{name, type, body}`.
/// Anything unparseable yields no facts.
pub fn parse_facts(text: &str) -> Vec<Fact> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    value
        .as_array()
        .map(|items| items.iter().filter_map(one_fact).collect())
        .unwrap_or_default()
}

fn one_fact(item: &serde_json::Value) -> Option<Fact> {
    let name = item.get("name")?.as_str()?;
    let body = item.get("body")?.as_str()?;
    if !is_valid_name(name) || body.trim().is_empty() {
        return None;
    }
    let kind = item
        .get("type")
        .and_then(|k| k.as_str())
        .map(one_line)
        .filter(|k| !k.is_empty())
        .unwrap_or_else(|| "fact".to_string());
    Some(Fact {
        name: name.to_string(),
        kind,
        body: body.trim().to_string(),
    })
}

/// Character-trigram Jaccard similarity over lowercase alphanumerics:
/// 1.0 for identical text, 0.0 when nothing overlaps.
pub fn similarity(a: &str, b: &str) -> f64 {
    let (x, y) = (trigrams(a), trigrams(b));
    if x.is_empty() && y.is_empty() {
        return 1.0;
    }
    if x.is_empty() || y.is_empty() {
        return 0.0;
    }
    let both = x.intersection(&y).count() as f64;
    both / (x.len() + y.len()) as f64 * 2.0
}

fn trigrams(text: &str) -> HashSet<String> {
    let chars: Vec<char> = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    chars.windows(3).map(|w| w.iter().collect()).collect()
}

fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Session {
    /// Runs extraction now (called from `Shutdown`); returns the saved facts
    /// and stashes them for `drain_extracted`.
    pub(crate) async fn extract_memory(&self) -> Result<Vec<Fact>, CoreError> {
        let transcript = {
            let inner = self.inner.lock().await;
            compact::transcript(&inner.history)
        };
        let route = self
            .route_for(Job::Memory, true)
            .await
            .map_err(|e| CoreError::Config {
                key: "jobs".into(),
                message: e.notice(),
            })?;
        let req = Request {
            tier: route.tier,
            job: Job::Memory,
            model: route.model.clone(),
            system: vec![SystemBlock {
                text: PROMPT.to_string(),
                cache: false,
            }],
            tools: vec![],
            messages: vec![Message {
                role: Role::User,
                content: vec![Content::Text { text: transcript }],
            }],
            effort: route.effort,
            max_tokens: MAX_FACT_TOKENS.min(route.max_tokens),
            thinking: route.thinking,
            cache_breakpoints: vec![],
            stop_sequences: vec![],
        };
        let (tx, mut rx) = mpsc::channel(64);
        let provider = self.provider.clone();
        let cancel = self.cancel_token();
        let join = tokio::spawn(async move { provider.stream(req, tx, cancel).await });
        let mut out = String::new();
        while let Some(ev) = rx.recv().await {
            if let ProviderEvent::TextDelta { text } = ev {
                out.push_str(&text);
            }
        }
        let usage = join
            .await
            .map_err(|_| CoreError::Interrupted)?
            .map_err(|error| CoreError::Provider { error })?;
        self.store
            .usage_insert(&cox_protocol::UsageRow {
                session_id: self.id,
                turn: 0,
                job: Job::Memory,
                tier: route.tier,
                provider: self.provider.id(),
                model: route.model,
                effort: Some(route.effort),
                usage,
            })
            .map_err(|error| CoreError::Store { error })?;
        if budget::counts(route.tier, self.config.budget.cheap_counts) {
            self.add_spend(usage.cost_usd).await;
        }
        let mut saved = Vec::new();
        let project = slug_for(&self.cwd);
        // `salience` (T33.21.1) may drop a low-scoring candidate before it
        // ever reaches the store; it cannot add or edit one.
        let facts = self.advise_salience(parse_facts(&out)).await;
        for fact in facts {
            if self.already_known(&fact).await {
                continue;
            }
            let path = format!("{}.md", fact.name);
            match self
                .store
                .memory_upsert(&project, &fact.name, &path, &fact.kind, &fact.body)
            {
                Ok(()) => {
                    self.emit(Event::Notice {
                        level: Level::Info,
                        text: format!("memory saved: {}", fact.name),
                    })
                    .await?;
                    saved.push(fact);
                }
                Err(error) => {
                    self.emit(Event::Notice {
                        level: Level::Warn,
                        text: format!("memory save of {} failed: {error}", fact.name),
                    })
                    .await?;
                }
            }
        }
        self.inner.lock().await.extracted.extend(saved.clone());
        Ok(saved)
    }

    /// Facts `extract_memory` saved this session, for surfaces to
    /// materialise as `.md` files (the core never touches them directly).
    pub async fn drain_extracted(&self) -> Vec<Fact> {
        std::mem::take(&mut self.inner.lock().await.extracted)
    }

    /// True when a stored fact already says this (similarity over 0.8).
    async fn already_known(&self, fact: &Fact) -> bool {
        let mut queries = vec![fact.name.replace('-', " ")];
        let head: String = fact
            .body
            .split_whitespace()
            .take(8)
            .collect::<Vec<_>>()
            .join(" ");
        if !head.is_empty() {
            queries.push(head);
        }
        for q in queries {
            let Ok(hits) = self.store.memory_search(&q, 3) else {
                continue;
            };
            if hits
                .iter()
                .any(|h| similarity(&fact.body, &h.snippet) > DEDUP_SIMILARITY)
            {
                return true;
            }
        }
        false
    }
}

/// Project slug for the store key (mirrors `cox_ext::memory::slug_for`;
/// crate direction forbids sharing the function).
fn slug_for(cwd: &std::path::Path) -> String {
    let mut dir = cwd.to_path_buf();
    let root = loop {
        if dir.join(".git").exists() {
            break dir;
        }
        if !dir.pop() {
            break cwd.to_path_buf();
        }
    };
    root.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .map(|s| {
            let slug: String = s
                .to_ascii_lowercase()
                .bytes()
                .map(|b| {
                    if b.is_ascii_lowercase() || b.is_ascii_digit() {
                        b as char
                    } else {
                        '-'
                    }
                })
                .collect();
            slug.trim_matches('-').to_string()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_extract_similarity_scores_trigrams() {
        assert_eq!(similarity("", ""), 1.0);
        assert_eq!(similarity("abc", ""), 0.0);
        assert_eq!(
            similarity("login goes through auth", "login goes through auth"),
            1.0
        );
        let near = similarity(
            "login goes through the auth module with session cookies attached",
            "login goes through the auth module with session tokens attached",
        );
        assert!(near > DEDUP_SIMILARITY, "near-dup: {near}");
        let far = similarity("login goes through auth", "widgets render on canvas");
        assert!(far < 0.3, "unrelated: {far}");
    }

    #[test]
    fn memory_extract_parses_fact_json() {
        let facts = parse_facts(
            r#"[{"name": "auth-flow", "type": "decision", "body": "Use auth.rs."},
                {"name": "BAD NAME", "body": "skipped"},
                {"name": "empty", "body": "  "},
                {"nope": true}]"#,
        );
        assert_eq!(
            facts,
            vec![Fact {
                name: "auth-flow".into(),
                kind: "decision".into(),
                body: "Use auth.rs.".into(),
            }]
        );
        assert!(parse_facts("not json").is_empty());
        assert!(parse_facts("{}").is_empty());
    }
}
