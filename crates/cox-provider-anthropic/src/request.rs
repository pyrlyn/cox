// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Pure translation of a provider-neutral [`Request`] into an Anthropic
//! Messages body. No I/O and no client state, so every rule that decides
//! cost — where `cache_control` lands, whether a thinking block is replayed,
//! which effort is asked for — is a snapshot test instead of a live call.
//!
//! **Wire types.** The body is built as [`wire::CreateMessageParams`], the
//! type generated from Anthropic's own spec (T30.12), so a field name, a
//! required field or an enum value (`effort`, `ttl`, image media type) that
//! does not match the spec is a compile error. Three things then happen on
//! the serialized JSON, each because the generated type cannot say it:
//! keys are put back in the order cox has always sent ([`wire_order`]),
//! `cache_control` is placed ([`place_breakpoints`]), and the raw-JSON
//! escape hatches ([`Raw`], `fallbacks`) fill in what the snapshot lacks.
//!
//! **Breakpoint indexing.** `Request.cache_breakpoints` are indices into the
//! concatenation `system ++ messages`: `i < system.len()` names
//! `system[i]`, anything above names `messages[i - system.len()]`. A system
//! breakpoint marks that text block; a message breakpoint marks that
//! message's *last* content block, because Anthropic caches the prefix up
//! to and including the marked block. An index that is out of range, or
//! that names a `SystemBlock` with `cache == false`, is skipped rather than
//! being an error: context assembly (plan.md §1.9) owns the layout, and a
//! stale index must never fail a turn.

use cox_protocol::errors::ProviderError;
use cox_protocol::types::{Content, Effort, Message, ModelId, Request, Role, Thinking};
use serde_json::{Map, Value, json};

use super::{CacheTtl, wire};

/// Anthropic accepts at most four `cache_control` breakpoints per request;
/// a fifth is a 400. cox plans for three (plan.md §1.9) and clamps here so
/// a caller's mistake costs a cache miss, not the turn.
pub const MAX_BREAKPOINTS: usize = 4;

/// The key order of each object cox sends. The generated types serialize
/// fields alphabetically (tag first); request bytes are part of the
/// cache-stable prefix, so the order fixed before T30.12 is restored.
/// Keys not listed keep their serialized order after the listed ones.
const BODY_ORDER: &[&str] = &[
    "model",
    "max_tokens",
    "messages",
    "stream",
    "output_config",
    "system",
    "tools",
    "tool_choice",
    "thinking",
    "stop_sequences",
];
const MESSAGE_ORDER: &[&str] = &["role", "content"];
/// One list serves every block kind: no two kinds disagree on an order.
const BLOCK_ORDER: &[&str] = &[
    "type",
    "id",
    "tool_use_id",
    "name",
    "text",
    "thinking",
    "signature",
    "input",
    "content",
    "is_error",
    "source",
];
const SOURCE_ORDER: &[&str] = &["type", "media_type", "data"];
const TOOL_ORDER: &[&str] = &["name", "description", "input_schema"];
const CACHE_CONTROL_ORDER: &[&str] = &["type", "ttl"];

/// The provider-level knobs [`build_body`] needs that are not part of the
/// `Request` itself.
#[derive(Debug, Clone, Copy)]
pub struct BuildCfg<'a> {
    /// TTL written into every `cache_control` block.
    pub ttl: CacheTtl,
    /// Whether to send `fallbacks: "default"` (needs the matching beta header).
    pub fallbacks: bool,
    /// The model that produced the `Content::Thinking` blocks currently in
    /// history. `Content` carries no `produced_by` of its own, and adding
    /// one would change the rollout schema every crate shares, so the
    /// caller — which saw the `ModelSwitched` event — passes it here.
    /// `None`, or a value different from `Request.model`, drops the blocks.
    pub thinking_model: Option<&'a ModelId>,
}

/// A value the snapshot's types cannot hold, written over the typed
/// placeholder at `messages[message].content[block].<key>` after
/// serialization. cox-core stores a tool input that was not valid JSON as
/// `null` (the spec allows only an object), and an image may carry a media
/// type the snapshot does not list yet; neither is cox's to rewrite.
struct Raw {
    message: usize,
    block: usize,
    key: &'static str,
    value: Value,
}

/// Translates a `Request` into the JSON body for `POST /v1/messages`.
///
/// Fails only if a generated type refuses to serialize, which none of the
/// ones used here can; the error exists so that is not a panic.
pub fn build_body(req: &Request, cfg: BuildCfg<'_>) -> Result<Value, ProviderError> {
    let mut raw = Vec::new();
    // The prefix rule, not a catalog row, says whether a model takes
    // adaptive thinking (T30.25), so it needs no `Catalog` here.
    let caps = cox_models::Capabilities {
        adaptive_thinking: Some(cox_models::supports_adaptive_thinking(&req.model.0)),
        ..cox_models::Capabilities::default()
    };
    let sent = cox_models::effort_for(cox_models::Api::Anthropic, req.effort, &caps);
    let params = wire::CreateMessageParams {
        model: wire::Model(req.model.0.clone()),
        max_tokens: u64::from(req.max_tokens),
        messages: req
            .messages
            .iter()
            .enumerate()
            .map(|(i, m)| wire::InputMessage {
                role: match m.role {
                    Role::User => wire::InputMessageRole::User,
                    Role::Assistant => wire::InputMessageRole::Assistant,
                },
                content: wire::InputMessageContent::Array(content_blocks(
                    i, m, req, &cfg, &mut raw,
                )),
            })
            .collect(),
        stream: Some(true),
        output_config: sent.map(|w| wire::OutputConfig {
            effort: Some(effort(w.effort)),
            format: None,
        }),
        system: (!req.system.is_empty()).then(|| {
            wire::CreateMessageParamsSystem::Array(
                req.system.iter().map(|b| system_block(&b.text)).collect(),
            )
        }),
        tools: req.tools.iter().map(tool).collect(),
        // Never `any`/`tool`: forced tool use is a 400 on the Fable/Mythos
        // tier, and cox routes the same request shape to every model.
        tool_choice: (!req.tools.is_empty()).then_some(wire::ToolChoice::Auto {
            disable_parallel_tool_use: None,
        }),
        thinking: (req.thinking == Thinking::Adaptive && sent.is_some_and(|w| w.adaptive_thinking))
            .then_some(wire::ThinkingConfigParam::Adaptive { display: None }),
        stop_sequences: req.stop_sequences.clone(),
        cache_control: None,
        container: None,
        inference_geo: None,
        metadata: None,
        service_tier: None,
        temperature: None,
        top_k: None,
        top_p: None,
    };
    let mut body = to_value(&params)?;
    wire_order(&mut body);
    for r in raw {
        if let Some(block) = body
            .pointer_mut(&format!("/messages/{}/content/{}", r.message, r.block))
            .and_then(Value::as_object_mut)
        {
            block.insert(r.key.into(), r.value);
        }
    }
    let mut cache_control = to_value(&wire::CacheControlEphemeral {
        ttl: Some(match cfg.ttl {
            CacheTtl::FiveMinutes => wire::CacheControlEphemeralTtl::X5m,
            CacheTtl::OneHour => wire::CacheControlEphemeralTtl::X1h,
        }),
        type_: "ephemeral".into(),
    })?;
    in_order(&mut cache_control, CACHE_CONTROL_ORDER);
    place_breakpoints(req, &cache_control, &mut body);
    // Raw JSON: `fallbacks` is a beta field the spec snapshot does not list
    // (`CreateMessageParams` is closed). The scalar form: Anthropic picks the
    // substitute by refusal category, so cox owes no migration when one is
    // deprecated.
    if cfg.fallbacks
        && let Some(obj) = body.as_object_mut()
    {
        obj.insert("fallbacks".into(), json!("default"));
    }
    Ok(body)
}

fn to_value<T: serde::Serialize>(v: &T) -> Result<Value, ProviderError> {
    serde_json::to_value(v).map_err(|e| ProviderError::BadRequest {
        message: format!("request body: {e}"),
    })
}

/// Restores cox's key order ([`BODY_ORDER`] and friends) on a serialized
/// body. Only the objects cox builds are touched: tool `input` and
/// `input_schema` are caller data and keep their own order.
fn wire_order(body: &mut Value) {
    in_order(body, BODY_ORDER);
    for_each_in(body, "system", |b| in_order(b, BLOCK_ORDER));
    for_each_in(body, "tools", |t| in_order(t, TOOL_ORDER));
    for_each_in(body, "messages", |m| {
        in_order(m, MESSAGE_ORDER);
        for_each_in(m, "content", |b| {
            in_order(b, BLOCK_ORDER);
            if let Some(source) = b.get_mut("source") {
                in_order(source, SOURCE_ORDER);
            }
        });
    });
}

fn for_each_in(v: &mut Value, key: &str, f: impl FnMut(&mut Value)) {
    if let Some(items) = v.get_mut(key).and_then(Value::as_array_mut) {
        items.iter_mut().for_each(f);
    }
}

fn in_order(v: &mut Value, order: &[&str]) {
    let Some(map) = v.as_object_mut() else {
        return;
    };
    let mut rest = std::mem::take(map);
    for key in order {
        if let Some(value) = rest.shift_remove(*key) {
            map.insert((*key).to_string(), value);
        }
    }
    map.extend(rest);
}

/// Sets `cache_control` on the blocks named by `Request.cache_breakpoints`.
/// It works on the serialized body, not the typed one, so a breakpoint on a
/// message's last block lands whatever that block's kind is, exactly as
/// before T30.12, and the key goes last in the block.
fn place_breakpoints(req: &Request, cache_control: &Value, body: &mut Value) {
    let mut placed = 0;
    for &i in &req.cache_breakpoints {
        if placed == MAX_BREAKPOINTS {
            break;
        }
        let target = if i < req.system.len() {
            if !req.system[i].cache {
                continue;
            }
            body.pointer_mut(&format!("/system/{i}"))
        } else {
            body.pointer_mut(&format!("/messages/{}/content", i - req.system.len()))
                .and_then(Value::as_array_mut)
                .and_then(|blocks| blocks.last_mut())
        };
        if let Some(block) = target.and_then(Value::as_object_mut) {
            block.insert("cache_control".into(), cache_control.clone());
            placed += 1;
        }
    }
}

fn system_block(text: &str) -> wire::RequestTextBlock {
    wire::RequestTextBlock {
        cache_control: None,
        citations: None,
        text: text.to_string(),
        type_: "text".into(),
    }
}

fn tool(t: &cox_protocol::types::ToolSpec) -> wire::CreateMessageParamsToolsItem {
    wire::CreateMessageParamsToolsItem::Tool(wire::Tool {
        name: t.name.clone(),
        description: Some(t.description.clone()),
        input_schema: wire::InputSchema(t.input_schema.clone()),
        allowed_callers: Vec::new(),
        cache_control: None,
        defer_loading: None,
        eager_input_streaming: None,
        input_examples: Vec::new(),
        strict: None,
        type_: None,
    })
}

fn text(text: String) -> wire::InputContentBlock {
    wire::InputContentBlock::Text {
        cache_control: None,
        citations: None,
        text,
    }
}

/// One message's content blocks. Several `Content::ToolResult`s in the same
/// user message become several `tool_result` blocks in that one message,
/// which is how Anthropic wants a parallel tool batch answered.
fn content_blocks(
    message: usize,
    m: &Message,
    req: &Request,
    cfg: &BuildCfg<'_>,
    raw: &mut Vec<Raw>,
) -> Vec<wire::InputContentBlock> {
    let mut blocks = Vec::new();
    for c in &m.content {
        let block = match c {
            Content::Text { text: t } => text(t.clone()),
            Content::ToolUse { id, name, input } => {
                let input = match input {
                    Value::Object(map) => map.clone(),
                    other => {
                        raw.push(Raw {
                            message,
                            block: blocks.len(),
                            key: "input",
                            value: other.clone(),
                        });
                        Map::new()
                    }
                };
                wire::InputContentBlock::ToolUse {
                    cache_control: None,
                    caller: None,
                    id: id.to_string(),
                    input,
                    name: name.clone(),
                    toolset_name: None,
                }
            }
            Content::ToolResult {
                call_id,
                content,
                is_error,
            } => wire::InputContentBlock::ToolResult {
                cache_control: None,
                content: Some(wire::RequestToolResultBlockContent::String(content.clone())),
                is_error: Some(*is_error),
                tool_use_id: call_id.to_string(),
                toolset_name: None,
            },
            Content::Image {
                media_type,
                data_b64,
            } => {
                let known = media_type.parse::<wire::Base64ImageSourceMediaType>();
                if known.is_err() {
                    raw.push(Raw {
                        message,
                        block: blocks.len(),
                        key: "source",
                        value: json!({"type": "base64", "media_type": media_type, "data": data_b64}),
                    });
                }
                wire::InputContentBlock::Image {
                    cache_control: None,
                    source: wire::RequestImageBlockSource::Base64 {
                        data: data_b64.clone(),
                        media_type: known.unwrap_or(wire::Base64ImageSourceMediaType::ImagePng),
                    },
                    transformations: None,
                }
            }
            // Microcompaction: the model sees the summary and the id it can
            // pass to `expand`, never the archived bytes.
            Content::Pointer { archive, summary } => {
                text(format!("[archived: {summary}; expand {}]", archive.id))
            }
            // A signature is bound to the model that produced it: replaying
            // one to a different model is at best ignored and at worst a
            // 400, so a block only survives a model switch by being dropped.
            Content::Thinking {
                text: thought,
                signature,
            } => match (signature, cfg.thinking_model) {
                (Some(sig), Some(produced_by)) if *produced_by == req.model => {
                    wire::InputContentBlock::Thinking {
                        signature: sig.clone(),
                        thinking: thought.clone(),
                    }
                }
                _ => continue,
            },
        };
        blocks.push(block);
    }
    blocks
}

/// Type conversion only: which level to send is `cox_models::effort_for`'s.
fn effort(e: Effort) -> wire::EffortLevel {
    match e {
        Effort::Low => wire::EffortLevel::Low,
        Effort::Medium => wire::EffortLevel::Medium,
        Effort::High => wire::EffortLevel::High,
        Effort::Xhigh => wire::EffortLevel::Xhigh,
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use cox_protocol::ids::{ArchiveId, CallId};
    use cox_protocol::types::{ArchiveRef, Concurrency, Job, Risk, SystemBlock, Tier, ToolSpec};

    use super::*;

    /// Shadows [`super::build_body`] so every call site, and with it each
    /// snapshot's recorded expression, reads as it did before it returned a
    /// `Result`.
    fn build_body(req: &Request, cfg: BuildCfg<'_>) -> Value {
        super::build_body(req, cfg).expect("the generated types always serialize")
    }

    /// Fixed ids so the snapshots are byte-stable across runs.
    fn call(n: u8) -> CallId {
        CallId::from_str(&format!("01ARZ3NDEKTSV4RRFFQ69G5FA{n}")).expect("valid ulid")
    }

    fn cfg(thinking_model: Option<&ModelId>) -> BuildCfg<'_> {
        BuildCfg {
            ttl: CacheTtl::FiveMinutes,
            fallbacks: true,
            thinking_model,
        }
    }

    fn base(model: &str) -> Request {
        Request {
            tier: Tier::Code,
            job: Job::Main,
            model: ModelId(model.into()),
            system: vec![
                SystemBlock {
                    text: "<tool specs>".into(),
                    cache: true,
                },
                SystemBlock {
                    text: "You are cox.".into(),
                    cache: true,
                },
                SystemBlock {
                    text: "date: 2026-09-02".into(),
                    cache: false,
                },
            ],
            tools: vec![ToolSpec {
                name: "read".into(),
                description: "Read a file".into(),
                input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
                deferred: false,
                risk: Risk::ReadOnly,
                concurrency: Concurrency::Parallel,
            }],
            messages: vec![],
            effort: Effort::High,
            max_tokens: 16384,
            thinking: Thinking::Adaptive,
            cache_breakpoints: vec![1],
            stop_sequences: vec![],
        }
    }

    fn user_text(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![Content::Text { text: text.into() }],
        }
    }

    #[test]
    fn anthropic_request_plain_text() {
        let mut req = base("claude-sonnet-5");
        req.messages = vec![user_text("what does cox-provider own?")];

        insta::assert_json_snapshot!(build_body(&req, cfg(None)));
    }

    /// T37.6 Check: an attached image reaches the wire as a base64 block.
    #[test]
    fn anthropic_request_user_image() {
        let mut req = base("claude-sonnet-5");
        req.messages = vec![Message {
            role: Role::User,
            content: vec![
                Content::Text {
                    text: "what is in this screenshot?".into(),
                },
                Content::Image {
                    media_type: "image/png".into(),
                    data_b64: "iVBORw0KGgo=".into(),
                },
            ],
        }];

        insta::assert_json_snapshot!(build_body(&req, cfg(None)));
    }

    #[test]
    fn anthropic_request_parallel_tool_results() {
        let mut req = base("claude-sonnet-5");
        req.messages = vec![
            user_text("read both files"),
            Message {
                role: Role::Assistant,
                content: vec![
                    Content::Text {
                        text: "Reading both.".into(),
                    },
                    Content::ToolUse {
                        id: call(1),
                        name: "read".into(),
                        input: json!({"path": "a.rs"}),
                    },
                    Content::ToolUse {
                        id: call(2),
                        name: "read".into(),
                        input: json!({"path": "b.rs"}),
                    },
                ],
            },
            // Both results ride in one user message, as Anthropic requires
            // for a parallel batch.
            Message {
                role: Role::User,
                content: vec![
                    Content::ToolResult {
                        call_id: call(1),
                        content: "fn a() {}".into(),
                        is_error: false,
                    },
                    Content::ToolResult {
                        call_id: call(2),
                        content: "no such file".into(),
                        is_error: true,
                    },
                ],
            },
        ];
        // system[1] (end of the stable prefix), the assistant turn, and the
        // tool-result turn: three of the four slots.
        req.cache_breakpoints = vec![1, 4, 5];

        let body = build_body(&req, cfg(None));
        assert_eq!(count_cache_control(&body), 3);
        insta::assert_json_snapshot!(body);
    }

    #[test]
    fn anthropic_request_after_compaction() {
        // The turn is on opus after a `/model opus` switch; the thinking
        // block in history was produced by sonnet, so it is dropped.
        let mut req = base("claude-opus-5");
        req.messages = vec![
            Message {
                role: Role::User,
                content: vec![Content::Text {
                    text: "Summary of earlier work: refactored the parser.".into(),
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![
                    Content::Thinking {
                        text: "the parser is recursive descent".into(),
                        signature: Some("sig-from-sonnet".into()),
                    },
                    Content::Text {
                        text: "Continuing.".into(),
                    },
                ],
            },
            Message {
                role: Role::User,
                content: vec![
                    Content::Pointer {
                        archive: ArchiveRef {
                            id: ArchiveId::from_str("01ARZ3NDEKTSV4RRFFQ69G5FB0")
                                .expect("valid ulid"),
                            bytes: 91_000,
                        },
                        summary: "bash cargo test: 91000 bytes, exit 0".into(),
                    },
                    Content::Text {
                        text: "now fix the failing test".into(),
                    },
                ],
            },
        ];
        req.cache_breakpoints = vec![1, 4];

        let body = build_body(&req, cfg(Some(&ModelId("claude-sonnet-5".into()))));
        let dumped = serde_json::to_string(&body).expect("serializes");
        assert!(
            !dumped.contains("sig-from-sonnet"),
            "stale thinking replayed"
        );
        insta::assert_json_snapshot!(body);
    }

    #[test]
    fn thinking_replayed_only_on_same_model() {
        let mut req = base("claude-sonnet-5");
        req.messages = vec![Message {
            role: Role::Assistant,
            content: vec![Content::Thinking {
                text: "thought".into(),
                signature: Some("sig".into()),
            }],
        }];

        let same = ModelId("claude-sonnet-5".into());
        let body = build_body(&req, cfg(Some(&same)));
        assert_eq!(
            body["messages"][0]["content"][0]["type"], "thinking",
            "same model must replay the block verbatim"
        );
        assert_eq!(body["messages"][0]["content"][0]["signature"], "sig");

        let other = ModelId("claude-opus-5".into());
        let body = build_body(&req, cfg(Some(&other)));
        assert_eq!(
            body["messages"][0]["content"].as_array().map(Vec::len),
            Some(0),
            "a block from another model must be dropped"
        );

        // Unknown provenance is treated like a switch: never guess.
        let body = build_body(&req, cfg(None));
        assert_eq!(
            body["messages"][0]["content"].as_array().map(Vec::len),
            Some(0)
        );
    }

    #[test]
    fn breakpoints_never_exceed_four() {
        let mut req = base("claude-sonnet-5");
        req.messages = (0..8).map(|i| user_text(&format!("turn {i}"))).collect();
        // Six valid indices, one pointing at a non-cacheable system block
        // and one past the end: the body must still carry at most four.
        req.cache_breakpoints = vec![0, 1, 2, 3, 4, 5, 6, 7, 99];

        let body = build_body(&req, cfg(None));
        assert_eq!(count_cache_control(&body), MAX_BREAKPOINTS);
        // The volatile system block never gets one, whatever the caller asks.
        assert!(body["system"][2].get("cache_control").is_none());
    }

    #[test]
    fn medium_effort_is_sent_and_adaptive_thinking_follows_the_model() {
        let mut req = base("claude-sonnet-5");
        req.effort = Effort::Medium;
        let body = build_body(&req, cfg(None));
        assert_eq!(body["output_config"]["effort"], "medium");
        assert_eq!(body["thinking"]["type"], "adaptive");

        // Not an adaptive-thinking model: the effort still goes out, the
        // `thinking` field does not.
        let mut req = base("claude-haiku-4-5");
        req.effort = Effort::Medium;
        let body = build_body(&req, cfg(None));
        assert_eq!(body["output_config"]["effort"], "medium");
        assert!(body.get("thinking").is_none());
    }

    fn count_cache_control(v: &Value) -> usize {
        match v {
            Value::Object(map) => {
                let here = usize::from(map.contains_key("cache_control"));
                here + map.values().map(count_cache_control).sum::<usize>()
            }
            Value::Array(items) => items.iter().map(count_cache_control).sum(),
            _ => 0,
        }
    }
}
