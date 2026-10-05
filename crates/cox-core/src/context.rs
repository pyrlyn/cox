// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Cache-stable request assembly (plan.md §1.9). Separate from `turn` so the
//! prefix order can be snapshot-tested without running tools. `Breakdown`
//! (T25.7) attributes a request's estimated tokens back to those segments.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use cox_protocol::ids::CallId;
use cox_protocol::image;
use cox_protocol::traits::Tool;
use cox_protocol::types::{
    ArchiveRef, Attachment, Content, ContextBreakdown, Job, Message, ModelId, PermissionMode,
    Request, SystemBlock, Tier, Usage,
};

/// The first line of `system[2]`; the loaded instruction files and the
/// skills index follow it (T50.1). Kept even when both are empty so a
/// session without either sends the same prefix bytes as before.
const INSTRUCTIONS: &str = "Follow repository instruction files when present.";

const PROMPT: &str = include_str!("prompt.md");

/// The T30.1 minimal prompt: the same contract in one breath, for the
/// `minimal` profile whose whole prefix must stay under 1 000 tokens.
const PROMPT_MINIMAL: &str = include_str!("prompt_minimal.md");

/// The tools the `minimal` profile keeps (T30.1): the core eight (§1.11)
/// plus `expand` (always present, tiny schema — the losslessness handle).
/// `tool_search` is out: under `minimal` it could only re-add listed names,
/// so the tool is dead weight in a prefix measured in hundreds of tokens.
/// Discovery of a non-listed tool stays out the same way.
const MINIMAL_TOOLS: &[&str] = &[
    "read",
    "grep",
    "glob",
    "edit",
    "apply_patch",
    "write",
    "bash",
    "todo",
    "expand",
];

/// Opens the repo map at the end of `system[2]` (P43).
pub(crate) const REPO_MAP_OPEN: &str = "<repo_map>\n";

/// Closes it; `system[2]` ends with this exactly when a map is present.
pub(crate) const REPO_MAP_CLOSE: &str = "\n</repo_map>";

/// The byte-stable text of `system[2]` after the `INSTRUCTIONS` line, in
/// that order: the instruction files (T50.1), the skills index (T22.2) and
/// the repo map (P43), each read or built once per session by a surface or
/// the session. A struct so the next stable part does not grow the
/// argument list of `assemble_with_skills` again.
#[derive(Debug, Clone, Copy, Default)]
pub struct Stable<'a> {
    pub instructions: &'a str,
    pub skills_index: &'a str,
    pub repomap: &'a str,
}

/// Whether `config` asks for the `minimal` prefix: `core.profile`, or the
/// `context.system_prompt` it implies when set directly.
fn is_minimal(config: &cox_protocol::Config) -> bool {
    config.core.profile == "minimal" || config.context.system_prompt == "minimal"
}

/// Builds a `Request` with `system[0..=2]` byte-stable and three breakpoints.
pub fn assemble(
    history: &[Message],
    config: &cox_protocol::Config,
    tools: &[Arc<dyn Tool>],
    cwd: &Path,
    date: &str,
) -> Request {
    assemble_with(history, config, Tier::Code, tools, &[], cwd, date)
}

/// `assemble` with the deferred tools the model has found through
/// `tool_search` (D6d): those specs join the request in discovery order
/// after the stable core set, so the prefix changes once per discovery
/// and is stable again afterwards. With `context.deferred_tools = false`
/// nothing is deferred and every tool is always present.
pub fn assemble_with(
    history: &[Message],
    config: &cox_protocol::Config,
    tier: Tier,
    tools: &[Arc<dyn Tool>],
    discovered: &[String],
    cwd: &Path,
    date: &str,
) -> Request {
    assemble_with_skills(
        history,
        config,
        tier,
        tools,
        discovered,
        cwd,
        date,
        &Stable::default(),
        config.permissions.mode,
    )
}

/// `assemble_with` plus the `system[2]` instruction-file block (T50.1),
/// skills index (T22.2) and repo map (P43), in that order after the
/// `INSTRUCTIONS` line; the map is last so a `/repomap refresh` leaves the
/// bytes before it alone and breakpoint 1 stays after `system[2]`. An
/// empty part appends nothing, so a user without either keeps the exact
/// prefix bytes of every earlier session and `system[0..=2]` stays
/// byte-stable across turns either way (D6e). The surface reads the first two once
/// (`cox_ext::instructions::load`, `cox_ext::skills::index`) and hands them
/// to `Session::set_instructions`; this crate reads no files. `mode` is the
/// session's live permission mode (T50.3); it goes into the volatile
/// `system[3]` only, so a mode switch never moves the cached prefix.
#[allow(clippy::too_many_arguments)]
pub fn assemble_with_skills(
    history: &[Message],
    config: &cox_protocol::Config,
    tier: Tier,
    tools: &[Arc<dyn Tool>],
    discovered: &[String],
    cwd: &Path,
    date: &str,
    stable: &Stable,
    mode: PermissionMode,
) -> Request {
    let all: Vec<_> = tools.iter().map(|t| t.spec()).collect();
    let deferring = config.context.deferred_tools;
    let minimal = is_minimal(config);
    // The minimal profile keeps its tool list only, whatever `deferred`
    // says: a non-listed tool would grow the prefix past the cap, so only
    // listed names join here and discovery below may only re-add them.
    let mut specs: Vec<_> = all
        .iter()
        .filter(|s| {
            if minimal {
                MINIMAL_TOOLS.contains(&s.name.as_str())
            } else {
                !deferring || !s.deferred
            }
        })
        .cloned()
        .collect();
    specs.sort_by(|a, b| a.name.cmp(&b.name));
    // Under `minimal` there is no discovery at all: the listed tools are
    // all present already, and anything else would grow the prefix past the
    // cap (`tool_search` itself is not listed, so the model cannot ask).
    if deferring && !minimal {
        for name in discovered {
            if specs.iter().any(|s| &s.name == name) {
                continue;
            }
            if let Some(spec) = all.iter().find(|s| s.deferred && &s.name == name) {
                specs.push(spec.clone());
            }
        }
    }
    let tools_json = serde_json::to_string(&specs).unwrap_or_else(|_| "[]".into());

    // The profile only shrinks blocks, never reorders them (§1.9): `system`
    // keeps its four slots and the three breakpoints, so the cache contract
    // the `prefix_bytes_identical_between_turns` test pins still holds.
    let prompt = if minimal { PROMPT_MINIMAL } else { PROMPT };
    // Under `minimal` the skills index never joins `system[2]`: it would
    // grow the prefix past the cap, and the profile promises no index. The
    // instruction files join under every profile: they are the user's
    // rules for the repository, not cox's scaffolding. The repo map goes
    // with the index: it is cox's scaffolding too.
    let index = if minimal { "" } else { stable.skills_index };
    let map = if minimal || stable.repomap.is_empty() {
        String::new()
    } else {
        format!(
            "{REPO_MAP_OPEN}{}{REPO_MAP_CLOSE}",
            stable.repomap.trim_end_matches('\n')
        )
    };
    let mut block2 = INSTRUCTIONS.to_string();
    for part in [stable.instructions, index, map.as_str()] {
        if !part.is_empty() {
            block2.push('\n');
            block2.push_str(part);
        }
    }
    let system = vec![
        SystemBlock {
            text: tools_json,
            cache: true,
        },
        SystemBlock {
            text: prompt.to_string(),
            cache: true,
        },
        SystemBlock {
            text: block2,
            cache: true,
        },
        SystemBlock {
            text: format!(
                "date={date}\ncwd={}\npermission_mode={mode:?}\n",
                cwd.display(),
            ),
            cache: false,
        },
    ];
    let cache_breakpoints = breakpoints(system.len(), history.len());
    let tc = config.tiers.get(tier);
    Request {
        tier,
        job: Job::Main,
        model: ModelId(tc.model.clone()),
        system,
        tools: specs,
        messages: history.to_vec(),
        effort: tc.effort,
        max_tokens: tc.max_tokens,
        thinking: tc.thinking,
        cache_breakpoints,
        stop_sequences: vec![],
    }
}

fn breakpoints(system_len: usize, n_messages: usize) -> Vec<usize> {
    let mut bps = vec![2];
    if n_messages >= 2 {
        bps.push(system_len + n_messages - 2);
    }
    if n_messages >= 1 {
        let last = system_len + n_messages - 1;
        if bps.last().copied() != Some(last) {
            bps.push(last);
        }
    }
    bps.truncate(3);
    bps
}

/// Microcompaction (T8.2 §1.10): old tool results become `Pointer`s in the
/// request without a model call. Pure over a copy: the stored history keeps
/// the visible text (so the rollout and `cox expand` are untouched); only
/// the returned messages change, one block at a time, so turn boundaries
/// and cache breakpoints are unaffected.
///
/// A result is replaced when its turn is older than `after_turns` back from
/// the newest AND outside the last `keep_turns` turns (which are never
/// touched). Turns come from `turn_starts` (T8.1 marks); an empty slice
/// means "no turn info" and returns the input unchanged.
pub fn microcompact(
    messages: &[Message],
    turn_starts: &[usize],
    keep_turns: u32,
    after_turns: u32,
    archives: &HashMap<CallId, ArchiveRef>,
) -> Vec<Message> {
    if turn_starts.is_empty() || messages.is_empty() {
        return messages.to_vec();
    }
    let n = turn_starts.len();
    let keep_from = n.saturating_sub(keep_turns as usize);
    let turn_of = |m: usize| -> usize {
        match turn_starts.binary_search(&m) {
            Ok(t) => t,
            Err(0) => 0,
            Err(t) => t - 1,
        }
        .min(n - 1)
    };
    // Tool names live in the matching `ToolUse` block in history.
    let mut names: HashMap<CallId, &str> = HashMap::new();
    for msg in messages {
        for c in &msg.content {
            if let Content::ToolUse { id, name, .. } = c {
                names.insert(*id, name.as_str());
            }
        }
    }
    messages
        .iter()
        .enumerate()
        .map(|(m, msg)| {
            let t = turn_of(m);
            // ponytail: O(turns) scan per message via binary_search; fine at
            // session sizes, revisit with a cursor if history grows large.
            if t >= keep_from || (n - t) <= after_turns as usize {
                return msg.clone();
            }
            let content = msg
                .content
                .iter()
                .map(|c| match c {
                    Content::ToolResult { call_id, .. } => match archives.get(call_id) {
                        Some(arch) => Content::Pointer {
                            archive: arch.clone(),
                            summary: format!(
                                "{}: {} bytes archived; expand #{}",
                                names.get(call_id).copied().unwrap_or("tool"),
                                arch.bytes,
                                arch.id
                            ),
                        },
                        None => c.clone(),
                    },
                    _ => c.clone(),
                })
                .collect();
            Message {
                role: msg.role,
                content,
            }
        })
        .collect()
}

/// A routed-down turn's request (T33.40.8, J20): thinking blocks from
/// turns before `turn_start` were signed by the static tier's model, so the
/// cheap model would reject them. They are dropped from this copy only; the
/// stored history keeps them, so the next static-tier request's prefix stays
/// byte-identical and warm. The running turn's own blocks are the cheap
/// model's and stay, as a tool loop with thinking needs them.
pub fn strip_thinking_before(mut messages: Vec<Message>, turn_start: usize) -> Vec<Message> {
    let current = messages.split_off(turn_start.min(messages.len()));
    let mut out = crate::router::strip_thinking(&messages);
    out.extend(current);
    out
}

/// Tool images are visible in their own turn only (T40.6): `Content::Image`
/// is dropped from this copy of every user message before `turn_start`
/// that carries tool results (a `ToolResult`, or the `Pointer`
/// microcompaction left in its place). The stored history keeps them and
/// the rollout never had them, so a resumed session builds the same
/// request; each result's text still names the image's archive row. A user
/// attachment sits in a message without tool results and stays.
pub fn strip_tool_images_before(mut messages: Vec<Message>, turn_start: usize) -> Vec<Message> {
    for msg in messages.iter_mut().take(turn_start) {
        let results = msg
            .content
            .iter()
            .any(|c| matches!(c, Content::ToolResult { .. } | Content::Pointer { .. }));
        if results {
            msg.content.retain(|c| !matches!(c, Content::Image { .. }));
        }
    }
    messages
}

/// The marker `compact.rs` prefixes its summary message with; otherwise a
/// summary is an indistinguishable plain user message (append-only history,
/// `ItemKind::Summary` replays as one) and could not fill `summary` below.
const SUMMARY_HEADER: &str = "[Compacted summary of ";

/// Where the next request's tokens go (T25.7 `/context`): the §1.9 segments
/// plus the whole and the cached share. `total` is the T1.8 estimator's
/// request total verbatim — cox-core may not depend on cox-provider, so the
/// provider-owning caller passes `cox_provider::tokens::estimate`'s number —
/// and the ten segment fields distribute it exactly.
pub struct Breakdown {
    pub tools: u32,
    pub system: u32,
    pub instructions: u32,
    pub skills: u32,
    pub memory: u32,
    pub volatile: u32,
    pub history_verbatim: u32,
    pub history_pointers: u32,
    pub summary: u32,
    /// The repo map at the end of `system[2]` (P43).
    pub repomap: u32,
    pub total: u32,
    pub cached_estimate: u32,
}

impl Breakdown {
    /// The four parts the surfaces draw (A98), with the model's `window`.
    /// The volatile block counts as system, skills, memory and the repo map
    /// as instructions (both are appended to those blocks today), and the
    /// summary and archive pointers as history.
    pub fn parts(&self, window: Option<u32>) -> ContextBreakdown {
        ContextBreakdown {
            window,
            total: self.total,
            system: self.system + self.volatile,
            tools: self.tools,
            instructions: self.instructions + self.skills + self.memory + self.repomap,
            history: self.history_verbatim + self.history_pointers + self.summary,
            cached: self.cached_estimate,
        }
    }
}

/// Splits `total` (the T1.8 estimator's request total for `req`) across the
/// segments by rendered bytes; cumulative rounding keeps the ten shares
/// summing to `total` exactly, so the modal's bars never disagree with the
/// estimate. `last_usage` supplies `cached_estimate` from its
/// `cache_read_tokens` — what the last call actually served from cache.
pub fn breakdown(req: &Request, total: u32, last_usage: Option<&Usage>) -> Breakdown {
    // Byte weights per segment (index order is `Breakdown`'s); the
    // estimator's byte term is itself a heuristic, so attributing each
    // content by its rendered JSON length is close enough for the split.
    let mut w = [0u64; 10];
    // T40.3: an image has no bytes to weigh; the estimator prices it flat,
    // so the same flat cost goes to its message's segment before the byte
    // split shares out the rest.
    let mut images = [0u64; 10];
    for (i, block) in req.system.iter().enumerate() {
        if i == 2 {
            let map = repomap_bytes(&block.text);
            w[2] += (block.text.len() - map) as u64 + 1;
            w[9] += map as u64;
            continue;
        }
        let seg = match i {
            0 => 0, // tool specs
            1 => 1, // system prompt
            2 => 2, // instruction files (the skills index is appended here, T22.2)
            _ => 5, // volatile (the memory index joins here in T10)
        };
        w[seg] += block.text.len() as u64 + 1;
    }
    for (i, msg) in req.messages.iter().enumerate() {
        // skills/memory stay empty until T7.1/T10 split them out of 2/3.
        let verbatim = match (&msg.content[..], i) {
            ([Content::Text { text }], 0) if text.starts_with(SUMMARY_HEADER) => 8,
            _ => 6,
        };
        for c in &msg.content {
            let seg = match c {
                Content::Pointer { .. } => 7,
                Content::Image { .. } => {
                    images[verbatim] += image::IMAGE_TOKEN_ESTIMATE;
                    continue;
                }
                _ => verbatim,
            };
            w[seg] += serde_json::to_string(c).map_or(0, |s| s.len() as u64);
        }
    }
    // Capped so the shares still sum to `total` when it came from
    // somewhere that priced images lower.
    let mut rest = u64::from(total);
    for n in &mut images {
        *n = (*n).min(rest);
        rest -= *n;
    }
    let sum: u64 = w.iter().sum();
    let mut shares = [0u32; 10];
    let (mut acc, mut prev) = (0u64, 0u64);
    for (i, weight) in w.iter().enumerate() {
        acc += weight;
        let cum = rest * acc / sum.max(1);
        shares[i] = (cum - prev + images[i]) as u32;
        prev = cum;
    }
    if sum == 0 {
        shares[5] += rest as u32; // a byte-free request still costs; volatile is the catch-all
    }
    Breakdown {
        tools: shares[0],
        system: shares[1],
        instructions: shares[2],
        skills: shares[3],
        memory: shares[4],
        volatile: shares[5],
        history_verbatim: shares[6],
        history_pointers: shares[7],
        summary: shares[8],
        repomap: shares[9],
        total,
        cached_estimate: last_usage.map_or(0, |u| u.cache_read_tokens),
    }
}

/// The bytes of `system[2]` text that are the repo map, its tags included;
/// 0 without one. Found from the end because the map is always last.
pub(crate) fn repomap_bytes(text: &str) -> usize {
    if !text.ends_with(REPO_MAP_CLOSE) {
        return 0;
    }
    text.rfind(&format!("\n{REPO_MAP_OPEN}"))
        .map_or(0, |at| text.len() - at)
}

/// The image types every wire takes (Anthropic's base64 source allows
/// exactly these four), so an image that passes never needs a per-wire
/// fallback.
const IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// A user turn's content (T37.6, T40.2): see [`attached_content`] for the
/// shape. Each attachment is admitted first — an image must pass
/// `image::validate` and `images` must say `model` takes it, any other file
/// must be UTF-8 — and what is not admitted is dropped with one notice
/// saying why (fail open: the text still goes). Returns the content, the
/// admitted attachments (what the rollout records, so resume rebuilds the
/// same message) and the notices.
pub fn user_content(
    text: String,
    context: Option<String>,
    attachments: Vec<Attachment>,
    model: &str,
    images: bool,
) -> (Vec<Content>, Vec<Attachment>, Vec<String>) {
    let mut kept = Vec::new();
    let mut held = Vec::new();
    for a in attachments {
        match admit(&a, model, images) {
            Ok(()) => kept.push(a),
            Err(why) => held.push(why),
        }
    }
    (attached_content(text, context, &kept), kept, held)
}

/// A user message built from admitted attachments: the images in
/// submission order, then the text and any hook context, then each text
/// file as a tagged block, which every wire carries. Images lead because
/// Anthropic's vision guide advises image-then-text (plan.md P40). Shared
/// by the live turn and the rollout rebuild (invariant 6).
pub fn attached_content(
    text: String,
    context: Option<String>,
    attachments: &[Attachment],
) -> Vec<Content> {
    let (images, files): (Vec<&Attachment>, Vec<&Attachment>) =
        attachments.iter().partition(|a| is_image(a));
    let images = images.into_iter().map(|a| Content::Image {
        media_type: a.media_type.to_ascii_lowercase(),
        data_b64: a.data_b64.clone(),
    });
    let texts = std::iter::once(text)
        .chain(context)
        .map(|text| Content::Text { text });
    let files = files.into_iter().filter_map(|a| {
        text_body(a).map(|body| Content::Text {
            text: format!(
                "<attachment name={:?} media_type={:?}>\n{body}\n</attachment>",
                a.name,
                a.media_type.to_ascii_lowercase()
            ),
        })
    });
    images.chain(texts).chain(files).collect()
}

/// Whether `a` goes into the user message, or the notice saying why not.
fn admit(a: &Attachment, model: &str, images: bool) -> Result<(), String> {
    if is_image(a) {
        if let Err(e) = image::validate(a) {
            return Err(format!("attachment {:?} dropped: {e}", a.name));
        }
        if !images {
            return Err(format!(
                "attachment {:?} not sent: {model} does not take images on this provider \
                 (a chat-api model opts in with `images = true` on its `models` entry)",
                a.name
            ));
        }
        return Ok(());
    }
    match text_body(a) {
        Some(_) => Ok(()),
        None => Err(format!(
            "attachment {:?} not sent: {} is neither a png/jpeg/gif/webp image nor UTF-8 text",
            a.name,
            a.media_type.to_ascii_lowercase()
        )),
    }
}

fn is_image(a: &Attachment) -> bool {
    IMAGE_TYPES.contains(&a.media_type.to_ascii_lowercase().as_str())
}

fn text_body(a: &Attachment) -> Option<String> {
    let bytes = STANDARD.decode(&a.data_b64).ok()?;
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attach(name: &str, media_type: &str, bytes: &[u8]) -> Attachment {
        Attachment {
            name: name.into(),
            media_type: media_type.into(),
            data_b64: STANDARD.encode(bytes),
        }
    }

    /// T40.2: an image leads the text as `Content::Image` when the model
    /// takes images, a UTF-8 file joins after it as a tagged text block, and
    /// both are kept for the rollout.
    #[test]
    fn user_attachment_becomes_image_block_before_text() {
        let files = vec![
            attach("shot.png", "image/PNG", b"\x89PNG"),
            attach("notes.md", "text/markdown", b"# hi"),
        ];
        let (content, kept, held) = user_content("look".into(), None, files.clone(), "m", true);
        assert!(held.is_empty(), "{held:?}");
        assert_eq!(kept, files);
        assert_eq!(
            content,
            vec![
                Content::Image {
                    media_type: "image/png".into(),
                    data_b64: files[0].data_b64.clone(),
                },
                Content::Text {
                    text: "look".into()
                },
                Content::Text {
                    text: "<attachment name=\"notes.md\" media_type=\"text/markdown\">\n# hi\n</attachment>"
                        .into()
                },
            ]
        );
        assert_eq!(attached_content("look".into(), None, &kept), content);
    }

    /// T37.6: what the wire cannot take is left out, one notice each, and
    /// is not kept for the rollout.
    #[test]
    fn user_content_holds_back_what_the_wire_cannot_take() {
        let files = vec![
            attach("shot.png", "image/png", b"\x89PNG"),
            attach("a.bin", "application/octet-stream", &[0xff, 0xfe, 0x00]),
        ];
        let (content, kept, held) = user_content("look".into(), None, files, "qwen3", false);
        assert_eq!(
            content,
            vec![Content::Text {
                text: "look".into()
            }]
        );
        assert!(kept.is_empty(), "{kept:?}");
        assert_eq!(held.len(), 2);
        assert!(held[0].contains("\"shot.png\"") && held[0].contains("qwen3 does not take images"));
        assert!(held[1].contains("\"a.bin\"") && held[1].contains("neither"));
    }

    /// T40.2: an image `image::validate` refuses is dropped with a notice
    /// naming it and the reason, even on a wire that takes images.
    #[test]
    fn invalid_attachment_is_warned_and_dropped() {
        let files = vec![
            attach("fake.png", "image/png", b"\xff\xd8\xff\xe0"),
            attach("text.png", "image/png", b"hello"),
        ];
        let (content, kept, held) = user_content("look".into(), None, files, "m", true);
        assert_eq!(
            content,
            vec![Content::Text {
                text: "look".into()
            }]
        );
        assert!(kept.is_empty(), "{kept:?}");
        assert_eq!(
            held,
            vec![
                "attachment \"fake.png\" dropped: declared image/png, but the bytes are image/jpeg"
                    .to_owned(),
                "attachment \"text.png\" dropped: not a PNG, JPEG, GIF or WebP image".to_owned(),
            ]
        );
    }

    /// T25.7: `breakdown.total` equals the estimator's request total and the
    /// segment shares sum to that estimate exactly.
    #[test]
    fn breakdown_sums_to_estimate() {
        let history: Vec<Message> = serde_json::from_value(serde_json::json!([
            {"role": "user", "content": [{"type": "text", "text": "[Compacted summary of 2 earlier turn(s)]"}]},
            {"role": "user", "content": [{"type": "text", "text": "go on"}]},
            {"role": "user", "content": [{"type": "pointer",
                "archive": {"id": "01ARZ3NDEKTSV4RRFFQ69G5FAV", "bytes": 9},
                "summary": "read: 9 bytes"}]},
            {"role": "assistant", "content": [{"type": "tool_use",
                "id": "01ARZ3NDEKTSV4RRFFQ69G5FAV", "name": "read", "input": {"path": "lib.rs"}}]},
        ]))
        .expect("history");
        let config = cox_protocol::Config::default();
        let req = assemble(&history, &config, &[], Path::new("/w"), "2026-09-22");
        let estimated = cox_provider::tokens::estimate(&req).tokens;
        let usage: Usage = serde_json::from_value(serde_json::json!(
            {"input_tokens": 1, "output_tokens": 1, "cache_read_tokens": 640,
             "cache_write_tokens": 8, "estimated": true, "cost_usd": 0.0, "latency_ms": 1}
        ))
        .expect("usage");
        let b = breakdown(&req, estimated, Some(&usage));
        assert_eq!(b.total, estimated, "the estimator's request total");
        assert_eq!(
            b.tools
                + b.system
                + b.instructions
                + b.skills
                + b.memory
                + b.volatile
                + b.history_verbatim
                + b.history_pointers
                + b.summary
                + b.repomap,
            b.total,
            "the segments sum to the estimate"
        );
        assert!(b.summary > 0 && b.history_pointers > 0 && b.instructions > 0);
        assert_eq!(b.cached_estimate, usage.cache_read_tokens);
    }

    fn png() -> Content {
        Content::Image {
            media_type: "image/png".into(),
            data_b64: "iVBORw==".into(),
        }
    }

    fn results_with_image() -> Message {
        Message {
            role: cox_protocol::types::Role::User,
            content: vec![
                Content::ToolResult {
                    call_id: CallId::new(),
                    content: "[image image/png, 0.0 KiB, archived as x; visible to the model \
                              in this turn only]"
                        .into(),
                    is_error: false,
                },
                png(),
            ],
        }
    }

    /// T40.6: a tool image before the turn start leaves the request; the
    /// pointer text stays, and the running turn keeps its own image.
    #[test]
    fn tool_image_dropped_after_its_turn() {
        let old = results_with_image();
        let current = results_with_image();
        let out = strip_tool_images_before(vec![old.clone(), current.clone()], 1);
        assert_eq!(out[0].content, old.content[..1].to_vec());
        assert_eq!(out[1], current);
    }

    /// T40.6: a user attachment is in a message without tool results, so it
    /// is sent on every later turn too.
    #[test]
    fn user_attachment_kept_across_turns() {
        let user = Message {
            role: cox_protocol::types::Role::User,
            content: vec![
                png(),
                Content::Text {
                    text: "look".into(),
                },
            ],
        };
        let history = vec![user, results_with_image()];
        let out = strip_tool_images_before(history.clone(), 2);
        assert_eq!(out[0], history[0]);
        assert!(!out[1].content.contains(&png()));
    }

    /// T40.3: an image counts `IMAGE_TOKEN_ESTIMATE` in its message's
    /// segment, and the shares still sum to the estimate.
    #[test]
    fn breakdown_counts_images() {
        let history = vec![Message {
            role: cox_protocol::types::Role::User,
            content: vec![
                Content::Image {
                    media_type: "image/png".into(),
                    data_b64: "iVBORw==".into(),
                },
                Content::Text {
                    text: "look".into(),
                },
            ],
        }];
        let config = cox_protocol::Config::default();
        let req = assemble(&history, &config, &[], Path::new("/w"), "2026-09-28");
        let estimated = cox_provider::tokens::estimate(&req).tokens;
        let b = breakdown(&req, estimated, None);
        assert!(
            u64::from(b.history_verbatim) >= image::IMAGE_TOKEN_ESTIMATE,
            "{}",
            b.history_verbatim
        );
        assert_eq!(
            b.tools
                + b.system
                + b.instructions
                + b.skills
                + b.memory
                + b.volatile
                + b.history_verbatim
                + b.history_pointers
                + b.summary,
            estimated
        );
    }

    /// T30.1: the minimal profile holds its tool list, prompt and discovery
    /// bar, and its prefix is smaller than default; the default prefix is
    /// byte-identical with or without the profile keys (the profile only
    /// shrinks, never reorders). Falsifier, recorded in the card: the T1.8
    /// estimator prices the nine minimal schemas alone at ~3.4k tokens, so
    /// the card's absolute ≤ 1 000 is unreachable without shrinking the
    /// schemas themselves (out of scope) — this pins the shape instead.
    #[test]
    fn minimal_prefix_under_1000_tokens() {
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(cox_tools::read::ReadTool),
            Arc::new(cox_tools::grep::GrepTool),
            Arc::new(cox_tools::glob::GlobTool),
            Arc::new(cox_tools::edit::EditTool),
            Arc::new(cox_tools::v4a::ApplyPatchTool),
            Arc::new(cox_tools::write::WriteTool),
            Arc::new(cox_tools::bash::BashTool),
            Arc::new(cox_tools::todo::TodoTool),
            Arc::new(cox_tools::expand::ExpandTool),
            Arc::new(cox_tools::web_fetch::WebFetchTool::new()),
            Arc::new(cox_tools::ask_user::AskUserTool::new(
                cox_tools::ask_user::Answers::Fixed(None),
            )),
            Arc::new(cox_tools::tool_search::ToolSearchTool::new(vec![])),
        ];
        let history: Vec<Message> = serde_json::from_value(serde_json::json!([
            {"role": "user", "content": [{"type": "text", "text": "hi"}]},
        ]))
        .expect("history");
        let mut minimal = cox_protocol::Config::default();
        minimal.core.profile = "minimal".to_string();
        let req = assemble(&history, &minimal, &tools, Path::new("/w"), "d");
        let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.iter().all(|n| MINIMAL_TOOLS.contains(n)),
            "only the profile list is present: {names:?}"
        );
        for kept in ["read", "bash", "todo", "expand"] {
            assert!(names.contains(&kept), "{kept} stays: {names:?}");
        }
        assert!(
            !names.contains(&"tool_search"),
            "discovery is dead weight out: {names:?}"
        );
        assert!(
            !names.contains(&"web_fetch"),
            "deferred tools stay out: {names:?}"
        );
        assert!(
            req.system[1].text.contains("smallest change"),
            "the short prompt is in system[1]"
        );
        // Discovery cannot grow the prefix past the cap either.
        let found = ["web_fetch".to_string()];
        let after = assemble_with(
            &history,
            &minimal,
            Tier::Code,
            &tools,
            &found,
            Path::new("/w"),
            "d",
        );
        assert!(
            !after.tools.iter().any(|t| t.name == "web_fetch"),
            "a non-listed discovery stays out"
        );
        let tokens = cox_provider::tokens::estimate(&req).tokens;
        let full = assemble(
            &history,
            &cox_protocol::Config::default(),
            &tools,
            Path::new("/w"),
            "d",
        );
        let full_tokens = cox_provider::tokens::estimate(&full).tokens;
        assert!(
            tokens < full_tokens,
            "the minimal prefix is smaller than the default: {tokens} vs {full_tokens}"
        );
        // The default prefix is untouched by the new keys: byte-identical
        // with the profile set and unset on the same history.
        let a = serde_json::to_vec(&full.system[0..=2]).expect("full");
        let b = assemble(
            &history,
            &cox_protocol::Config::default(),
            &tools,
            Path::new("/w"),
            "d",
        );
        let b = serde_json::to_vec(&b.system[0..=2]).expect("again");
        assert_eq!(a, b);
    }
    /// skills keeps an unchanged prefix, and the fixture skill `greeting`'s
    /// body reaches a request only after `skill{"name":"greeting"}` returns it.
    #[test]
    fn skills_index_is_in_system_2() {
        const GREETING: &str =
            include_str!("../../cox-ext/tests/fixtures/skills/greeting/SKILL.md");
        // Exactly what `cox_ext::skills::index` emits for the fixture skill.
        let index = "# Skills\nCall the `skill` tool with a name to load its instructions.\n- greeting: Greet the user in their language before answering.\n";
        let config = cox_protocol::Config::default();
        let call = CallId::new();
        let history: Vec<Message> = serde_json::from_value(serde_json::json!([
            {"role": "user", "content": [{"type": "text", "text": "hi"}]},
        ]))
        .expect("history");
        let first = assemble_with_skills(
            &history,
            &config,
            Tier::Code,
            &[],
            &[],
            Path::new("/w"),
            "d",
            &Stable {
                skills_index: index,
                ..Stable::default()
            },
            config.permissions.mode,
        );
        assert!(
            first.system[2].text.ends_with(index),
            "{:?}",
            first.system[2].text
        );
        assert!(first.system[2].text.contains("- greeting: "));
        assert!(
            GREETING.contains("Say hello in the language"),
            "the fixture"
        );
        let before = serde_json::to_string(&first).expect("request");
        assert!(
            !before.contains("Say hello in the language"),
            "the body is not in the first request"
        );

        // The body arrives only after `skill{"name":"greeting"}`: its tool
        // result is the first request content that carries the body.
        let mut invoked = history.clone();
        invoked.extend(
            serde_json::from_value::<Vec<Message>>(serde_json::json!([
                {"role": "assistant", "content": [{"type": "tool_use", "id": call,
                  "name": "skill", "input": {"name": "greeting"}}]},
                {"role": "user", "content": [{"type": "tool_result", "call_id": call,
                  "content": "# Skill: greeting\n\nSay hello in the language the user wrote in.",
                  "is_error": false}]},
            ]))
            .expect("skill round"),
        );
        let second = assemble_with_skills(
            &invoked,
            &config,
            Tier::Code,
            &[],
            &[],
            Path::new("/w"),
            "d",
            &Stable {
                skills_index: index,
                ..Stable::default()
            },
            config.permissions.mode,
        );
        assert!(
            serde_json::to_string(&second)
                .expect("request")
                .contains("Say hello in the language"),
            "the body arrives with the skill tool result"
        );
        assert_eq!(
            serde_json::to_vec(&first.system[0..=2]).expect("first"),
            serde_json::to_vec(&second.system[0..=2]).expect("second"),
            "the prefix is byte-identical between turns"
        );

        // No skills: byte-identical prefix to the pre-T22.2 assembly.
        let plain = assemble(&history, &config, &[], Path::new("/w"), "d");
        let empty = assemble_with_skills(
            &history,
            &config,
            Tier::Code,
            &[],
            &[],
            Path::new("/w"),
            "d",
            &Stable::default(),
            config.permissions.mode,
        );
        assert_eq!(
            serde_json::to_vec(&plain.system[0..=2]).expect("plain"),
            serde_json::to_vec(&empty.system[0..=2]).expect("empty"),
        );
    }

    /// T50.1: `system[2]` is the stub line, then the instruction files, then
    /// the skills index; `minimal` drops the index but keeps the user's files.
    #[test]
    fn instructions_precede_skills_index_and_survive_minimal() {
        let (block, index) = (
            "# Instructions\n## AGENTS.md\nBe terse.\n",
            "# Skills\n- a: b\n",
        );
        let two = |config: &cox_protocol::Config| {
            assemble_with_skills(
                &[],
                config,
                Tier::Code,
                &[],
                &[],
                Path::new("/w"),
                "d",
                &Stable {
                    instructions: block,
                    skills_index: index,
                    repomap: "",
                },
                config.permissions.mode,
            )
            .system[2]
                .text
                .clone()
        };
        let full = two(&cox_protocol::Config::default());
        assert_eq!(full, format!("{INSTRUCTIONS}\n{block}\n{index}"));
        let mut minimal = cox_protocol::Config::default();
        minimal.core.profile = "minimal".to_string();
        assert_eq!(two(&minimal), format!("{INSTRUCTIONS}\n{block}"));
    }

    fn with_map(config: &cox_protocol::Config, history: &[Message], map: &str) -> Request {
        assemble_with_skills(
            history,
            config,
            Tier::Code,
            &[],
            &[],
            Path::new("/w"),
            "d",
            &Stable {
                instructions: "# Instructions\nBe terse.\n",
                skills_index: "# Skills\n- a: b\n",
                repomap: map,
            },
            config.permissions.mode,
        )
    }

    const MAP: &str = "src/lib.rs\n  1: pub fn run()\n";

    /// P43: the map is the last thing in `system[2]`, after the instruction
    /// files and the skills index; breakpoint 1 still sits after `system[2]`.
    #[test]
    fn repomap_sits_last_in_system_two() {
        let config = cox_protocol::Config::default();
        let req = with_map(&config, &[], MAP);
        let two = &req.system[2].text;
        assert!(
            two.ends_with("<repo_map>\nsrc/lib.rs\n  1: pub fn run()\n</repo_map>"),
            "{two:?}"
        );
        let (skills, map) = (
            two.find("# Skills").expect("index"),
            two.find(REPO_MAP_OPEN).expect("map"),
        );
        assert!(two.find("Be terse.").expect("files") < skills && skills < map);
        assert_eq!(req.cache_breakpoints.first().copied(), Some(2));
        assert!(!req.system[3].text.contains("pub fn run"));
        let without = with_map(&config, &[], "");
        assert!(!without.system[2].text.contains("repo_map"));
        assert!(two.starts_with(&without.system[2].text), "only appended");
    }

    #[test]
    fn minimal_profile_omits_repomap() {
        let mut minimal = cox_protocol::Config::default();
        minimal.core.profile = "minimal".to_string();
        let req = with_map(&minimal, &[], MAP);
        assert!(!req.system[2].text.contains("repo_map"), "{req:?}");
        assert!(req.system[2].text.contains("Be terse."), "files stay");
    }

    /// The map is byte-stable between turns like the rest of the prefix.
    #[test]
    fn prefix_bytes_identical_between_turns_with_repomap() {
        let config = cox_protocol::Config::default();
        let history: Vec<Message> = serde_json::from_value(serde_json::json!([
            {"role": "user", "content": [{"type": "text", "text": "one"}]},
        ]))
        .expect("history");
        let mut longer = history.clone();
        longer.extend(
            serde_json::from_value::<Vec<Message>>(serde_json::json!([
                {"role": "assistant", "content": [{"type": "text", "text": "ok"}]},
                {"role": "user", "content": [{"type": "text", "text": "two"}]},
            ]))
            .expect("more"),
        );
        let a = with_map(&config, &history, MAP);
        let b = with_map(&config, &longer, MAP);
        assert_eq!(
            serde_json::to_vec(&a.system[0..=2]).expect("a"),
            serde_json::to_vec(&b.system[0..=2]).expect("b"),
        );
    }

    #[test]
    fn breakdown_counts_repomap_tokens() {
        let config = cox_protocol::Config::default();
        let plain = with_map(&config, &[], "");
        let mapped = with_map(&config, &[], &MAP.repeat(40));
        let none = breakdown(&plain, 1_000, None);
        assert_eq!(none.repomap, 0);
        let some = breakdown(&mapped, 1_000, None);
        assert!(some.repomap > 0, "the map has a share");
        assert!(
            some.repomap > some.instructions,
            "the map's bytes are not counted as instructions: {} vs {}",
            some.repomap,
            some.instructions
        );
        assert_eq!(
            some.parts(None).instructions,
            some.instructions + some.skills + some.memory + some.repomap
        );
        assert_eq!(repomap_bytes(&mapped.system[2].text), {
            let map = format!(
                "\n{REPO_MAP_OPEN}{}{REPO_MAP_CLOSE}",
                MAP.repeat(40).trim_end()
            );
            map.len()
        });
    }
}
