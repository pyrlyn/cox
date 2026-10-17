// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Refine proposals (T66.6, P66): one cheap `memory`-job call turns the
//! user's `/refine` text, a digest of the current harness entries and the
//! transcript tail into a validated list of edits to prompt notes, memory,
//! skills and subagents. Separate from the turn loop because it is a side
//! request: it goes through `side_call`, so it never touches the history or
//! the cache-stable prefix, and its cost is one `usage` row like any other.
//!
//! The answer is untrusted model output, so it is parsed into typed edits
//! and any violation rejects the whole proposal with a reason. Nothing here
//! writes a file: applying, backing up and recording an edit belong to the
//! session layer (T66.5, T66.7). The base prompt (`prompt.md`,
//! `prompt_minimal.md`) is an `include_str!` constant in `context.rs` and
//! no edit can name it.

use std::collections::HashSet;

use cox_protocol::types::Job;
use serde_json::Value;

use crate::compact;
use crate::memory_extract::is_valid_name;
use crate::session::Session;
use crate::side::Side;

const PROMPT: &str = include_str!("prompts/refine.md");

/// More edits than this is a rewrite, not a refine; a reviewer cannot
/// judge it at a glance.
pub const MAX_EDITS: usize = 5;

/// An entry larger than this would crowd the volatile `system[3]` slot the
/// digest is rendered into.
pub const MAX_CONTENT_BYTES: usize = 4096;

/// How much of the transcript's end the call reads: the recent turns are
/// the evidence, and older ones cost tokens without changing the edits.
const TAIL_BYTES: usize = 24_000;

/// Room for five full entries plus evidence and reasons; an answer cut
/// short is malformed JSON and is rejected rather than half-applied.
const MAX_ANSWER_TOKENS: u32 = 8192;
const MAX_ANSWER_CHARS: usize = 32_768;

/// Ids that name the base system prompt. The base prompt is not a harness
/// entry at all, but a model asked to "improve the prompt" reaches for
/// these names, and an entry under one would read as a replacement.
const BASE_PROMPT_IDS: &[&str] = &["prompt", "prompt-minimal", "base", "system"];

/// The four refinable kinds; there is deliberately no kind for the base
/// prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A standing instruction rendered after the cached prefix.
    PromptNote,
    /// A durable project fact.
    Memory,
    /// A reusable procedure (`SKILL.md` body).
    Skill,
    /// A delegated agent's definition.
    Subagent,
}

impl Kind {
    /// The wire and on-disk name (`harness/<kind>/<id>.md`).
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::PromptNote => "prompt-note",
            Kind::Memory => "memory",
            Kind::Skill => "skill",
            Kind::Subagent => "subagent",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        [Kind::PromptNote, Kind::Memory, Kind::Skill, Kind::Subagent]
            .into_iter()
            .find(|kind| kind.as_str() == text)
    }
}

/// What an edit does to its entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// A new entry.
    Create,
    /// Replaces an entry's whole content.
    Update,
    /// Removes an entry.
    Delete,
}

impl Action {
    /// The wire name, as `refine_events.edits` stores it.
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Create => "create",
            Action::Update => "update",
            Action::Delete => "delete",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        [Action::Create, Action::Update, Action::Delete]
            .into_iter()
            .find(|action| action.as_str() == text)
    }
}

/// One validated edit: `content` is present exactly for create and update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// What happens to the entry.
    pub action: Action,
    /// Which of the four kinds the entry is.
    pub kind: Kind,
    /// `[a-z0-9-]{1,64}`, never a base-prompt name.
    pub id: String,
    /// The whole new entry, at most [`MAX_CONTENT_BYTES`].
    pub content: Option<String>,
    /// The model's reason for this one edit.
    pub reason: String,
}

/// A proposal that passed every check, ready for review; empty `edits`
/// means the model found nothing worth changing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    /// The model's rationale for the whole refine.
    pub evidence: String,
    /// At most [`MAX_EDITS`], no entry twice.
    pub edits: Vec<Edit>,
}

/// Outcome `rejected`: the proposal yields no edits. `reason` is written
/// here, never copied from the model's answer, so it is safe to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    /// Why the proposal was refused.
    pub reason: String,
}

fn rejected(reason: impl Into<String>) -> Rejected {
    Rejected {
        reason: reason.into(),
    }
}

/// One edit's fields as strings, not enums, so a bad value gets this
/// module's reason instead of a parser message quoting the model's text.
struct WireEdit<'a> {
    action: &'a str,
    kind: &'a str,
    id: &'a str,
    content: Option<&'a str>,
    reason: &'a str,
}

/// Parses and validates the model's answer. Any malformed or out-of-bounds
/// part rejects the whole proposal: a partial apply would leave the
/// harness in a state nobody reviewed.
pub fn parse(answer: &str) -> Result<Proposal, Rejected> {
    let value: Value = serde_json::from_str(unfenced(answer)).map_err(|error| {
        rejected(format!(
            "malformed proposal: line {}, column {}",
            error.line(),
            error.column()
        ))
    })?;
    let evidence = value
        .get("evidence")
        .and_then(Value::as_str)
        .ok_or_else(|| rejected("malformed proposal: no `evidence` string"))?;
    let raw_edits = value
        .get("edits")
        .and_then(Value::as_array)
        .ok_or_else(|| rejected("malformed proposal: no `edits` array"))?;
    if raw_edits.len() > MAX_EDITS {
        return Err(rejected(format!(
            "{} edits, at most {MAX_EDITS} allowed",
            raw_edits.len()
        )));
    }
    let mut seen = HashSet::new();
    let mut edits = Vec::with_capacity(raw_edits.len());
    for (index, item) in raw_edits.iter().enumerate() {
        let number = index + 1;
        let raw = wire_edit(item).map_err(|field| {
            rejected(format!(
                "malformed proposal: edit {number} has no `{field}` string"
            ))
        })?;
        let edit = validate(raw).map_err(|why| rejected(format!("edit {number}: {why}")))?;
        if !seen.insert((edit.kind, edit.id.clone())) {
            return Err(rejected(format!(
                "edit {number}: {} `{}` is edited twice",
                edit.kind.as_str(),
                edit.id
            )));
        }
        edits.push(edit);
    }
    Ok(Proposal {
        evidence: evidence.trim().to_owned(),
        edits,
    })
}

/// The edit's fields, or the name of the first one missing or not a
/// string. `content` may be absent or `null`, which both mean none.
fn wire_edit(item: &Value) -> Result<WireEdit<'_>, &'static str> {
    let text = |field: &'static str| item.get(field).and_then(Value::as_str).ok_or(field);
    let content = match item.get("content") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_str().ok_or("content")?),
    };
    Ok(WireEdit {
        action: text("action")?,
        kind: text("kind")?,
        id: text("id")?,
        content,
        reason: text("reason")?,
    })
}

fn validate(raw: WireEdit<'_>) -> Result<Edit, &'static str> {
    // Checked before the kind and id rules so a base-prompt target gets
    // the reason that says why, not "unknown kind".
    if names_base_prompt(raw.kind) || names_base_prompt(raw.id) {
        return Err("the base system prompt is not editable");
    }
    let kind = Kind::parse(raw.kind).ok_or("unknown kind")?;
    let action = Action::parse(raw.action).ok_or("unknown action")?;
    if !is_valid_name(raw.id) {
        return Err("id must be 1-64 characters of a-z, 0-9 and -");
    }
    let content = match (action, raw.content) {
        (Action::Delete, None) => None,
        (Action::Delete, Some(_)) => return Err("a delete carries no content"),
        (_, None) => return Err("create and update need content"),
        (_, Some(text)) if text.trim().is_empty() => {
            return Err("create and update need content");
        }
        (_, Some(text)) if text.len() > MAX_CONTENT_BYTES => {
            return Err("content is over 4096 bytes");
        }
        (_, Some(text)) => Some(text.to_owned()),
    };
    Ok(Edit {
        action,
        kind,
        id: raw.id.to_owned(),
        content,
        reason: raw.reason.trim().to_owned(),
    })
}

fn names_base_prompt(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    BASE_PROMPT_IDS.contains(&name.as_str())
        || name.contains("base-prompt")
        || name.contains("system-prompt")
}

/// The answer without the code fence a model wraps JSON in despite the
/// prompt; the JSON itself is still parsed strictly.
fn unfenced(answer: &str) -> &str {
    let text = answer.trim();
    let Some(body) = text.strip_prefix("```") else {
        return text;
    };
    let body = body.strip_suffix("```").unwrap_or(body);
    match body.split_once('\n') {
        Some((tag, rest)) if !tag.contains('{') => rest.trim(),
        _ => body.trim(),
    }
}

/// The last `keep` bytes of `text`, cut on a char boundary.
fn tail(text: &str, keep: usize) -> &str {
    if text.len() <= keep {
        return text;
    }
    &text[text.ceil_char_boundary(text.len() - keep)..]
}

impl Session {
    /// Asks the `memory` job for a refine proposal. `text` is the user's
    /// `/refine` request and `digest` the current entries, which only the
    /// session layer can read. Writes nothing but the call's `usage` row.
    pub async fn propose_refine(&self, text: &str, digest: &str) -> Result<Proposal, Rejected> {
        let (transcript, turn) = {
            let inner = self.inner.lock().await;
            (compact::transcript(&inner.history), inner.turn_seq)
        };
        let digest = match digest.trim() {
            "" => "(no entries yet)",
            digest => digest,
        };
        let message = format!(
            "<request>\n{}\n</request>\n<digest>\n{digest}\n</digest>\n\
             <transcript_tail>\n{}\n</transcript_tail>",
            text.trim(),
            tail(&transcript, TAIL_BYTES)
        );
        let answer = self
            .side_call(Side {
                job: Job::Memory,
                turn,
                system: PROMPT,
                text: &message,
                max_tokens: MAX_ANSWER_TOKENS,
                max_chars: MAX_ANSWER_CHARS,
            })
            .await
            .ok_or_else(|| rejected("the refine call failed or answered nothing"))?;
        parse(&answer)
    }
}
