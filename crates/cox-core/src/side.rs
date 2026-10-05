// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! One-shot side requests: a background job's own tiny prompt (`/init`'s
//! README summary, the session title), sent outside the conversation so
//! the cache-stable prefix and the history never see it, and recorded in
//! the ledger like every other call. Separate so each job states only its
//! prompt, not the routing, streaming and ledger steps they share.

use cox_protocol::types::{Content, Job, Message, ProviderEvent, Request, Role, SystemBlock};
use tokio::sync::mpsc;

use crate::budget;
use crate::session::Session;

/// What one side request asks: the job it is billed to, its system prompt,
/// the one user message, and how much of the answer is kept.
pub(crate) struct Side<'a> {
    /// The job the router picks a tier for and the ledger row carries.
    pub job: Job,
    /// The ledger row's turn number.
    pub turn: u32,
    /// The job's own instructions.
    pub system: &'a str,
    /// The single user message.
    pub text: &'a str,
    /// Upper bound on output tokens, clamped to the route's own.
    pub max_tokens: u32,
    /// The answer is cut to this many characters.
    pub max_chars: usize,
}

impl Session {
    /// Runs `side` on its job's route and returns the trimmed answer.
    /// `None` on any failure — logged, never raised — so the caller falls
    /// back and the turn it follows never fails because of it.
    pub(crate) async fn side_call(&self, side: Side<'_>) -> Option<String> {
        let job = side.job.clone();
        let route = match self.route_for(job.clone(), true).await {
            Ok(route) => route,
            Err(error) => {
                tracing::warn!(job = ?job, error = ?error, "side request not routed");
                return None;
            }
        };
        let req = Request {
            tier: route.tier,
            job: job.clone(),
            model: route.model.clone(),
            system: vec![SystemBlock {
                text: side.system.to_string(),
                cache: false,
            }],
            tools: vec![],
            messages: vec![Message {
                role: Role::User,
                content: vec![Content::Text {
                    text: side.text.to_string(),
                }],
            }],
            effort: route.effort,
            max_tokens: side.max_tokens.min(route.max_tokens),
            thinking: route.thinking,
            cache_breakpoints: vec![],
            stop_sequences: vec![],
        };
        let (tx, mut rx) = mpsc::channel(64);
        let provider = self.provider.clone();
        let cancel = self.cancel_token();
        let join = tokio::spawn(async move { provider.stream(req, tx, cancel).await });
        let mut out = String::new();
        // Drained to the end, not cut: the provider reports usage only for
        // a stream it finished, and `max_tokens` already bounds it.
        while let Some(ev) = rx.recv().await {
            if let ProviderEvent::TextDelta { text } = ev
                && out.len() < side.max_chars
            {
                out.push_str(&text);
            }
        }
        let usage = match join.await {
            Ok(Ok(usage)) => usage,
            Ok(Err(error)) => {
                tracing::warn!(job = ?job, error = %error, "side request failed");
                return None;
            }
            Err(error) => {
                tracing::warn!(job = ?job, error = %error, "side request task failed");
                return None;
            }
        };
        let row = cox_protocol::UsageRow {
            session_id: self.id,
            turn: side.turn,
            job: job.clone(),
            tier: route.tier,
            provider: self.provider.id(),
            model: route.model,
            effort: Some(route.effort),
            usage,
        };
        if let Err(error) = self.store.usage_insert(&row) {
            tracing::warn!(job = ?job, error = %error, "side request not recorded");
            return None;
        }
        if budget::counts(route.tier, self.config.budget.cheap_counts) {
            self.add_spend(usage.cost_usd).await;
        }
        let answer: String = out.trim().chars().take(side.max_chars).collect();
        (!answer.is_empty()).then_some(answer)
    }
}
