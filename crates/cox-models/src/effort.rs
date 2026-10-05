// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The one effort map (plan.md T30.26; `docs/design/providers.md` § "Target
//! shape" item 4): which effort, if any, each wire sends for a request.
//! It lives next to the catalog because the answer depends on what a model
//! row declares, and so no wire decides for itself whether to send one.
//! Each wire still turns the returned [`Effort`] into its own generated
//! enum; that is a type conversion the compiler checks, not a policy.

use cox_protocol::types::Effort;

use crate::catalog::Capabilities;

/// The request shape a provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Api {
    /// Anthropic Messages: `output_config.effort` and `thinking`.
    Anthropic,
    /// OpenAI Responses: `reasoning.effort`.
    Responses,
    /// OpenAI Chat Completions and the servers that speak it: top-level
    /// `reasoning_effort`.
    Chat,
    /// TypeSafe Jev: a decision model with no effort field.
    Jev,
}

/// What a wire sends for a request's effort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireEffort {
    /// The level to send, as the router already clamped it.
    pub effort: Effort,
    /// Anthropic only: send `thinking: {"type": "adaptive"}` when the tier
    /// asks for thinking. Always `false` on the other wires.
    pub adaptive_thinking: bool,
}

/// The effort `api` sends for `effort` on a model with `caps`, or `None`
/// when that wire sends no effort field at all.
///
/// - Anthropic always sends `output_config.effort`; adaptive thinking
///   follows `caps.adaptive_thinking`.
/// - Responses always sends `reasoning.effort`.
/// - Chat sends `reasoning_effort` only when the row declares
///   `reasoning_effort_param`: OpenAI's Chat API documents the field, but
///   LM Studio's compatible endpoint does not list it (research.md §4.3.3).
/// - Jev has no effort field.
pub fn effort_for(api: Api, effort: Effort, caps: &Capabilities) -> Option<WireEffort> {
    let plain = WireEffort {
        effort,
        adaptive_thinking: false,
    };
    match api {
        Api::Anthropic => Some(WireEffort {
            adaptive_thinking: caps.adaptive_thinking == Some(true),
            ..plain
        }),
        Api::Responses => Some(plain),
        Api::Chat => (caps.reasoning_effort_param == Some(true)).then_some(plain),
        Api::Jev => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Effort; 4] = [Effort::Low, Effort::Medium, Effort::High, Effort::Xhigh];

    fn caps(adaptive: Option<bool>, param: Option<bool>) -> Capabilities {
        Capabilities {
            adaptive_thinking: adaptive,
            reasoning_effort_param: param,
            ..Capabilities::default()
        }
    }

    /// `(api, caps.adaptive_thinking, caps.reasoning_effort_param, sent)`;
    /// `sent` is `None` when the wire sends no effort, `Some(t)` when it
    /// sends the level with adaptive thinking `t`.
    type Row = (Api, Option<bool>, Option<bool>, Option<bool>);

    const TABLE: &[Row] = &[
        (Api::Anthropic, None, None, Some(false)),
        (Api::Anthropic, Some(false), None, Some(false)),
        (Api::Anthropic, Some(true), None, Some(true)),
        (Api::Anthropic, None, Some(true), Some(false)),
        (Api::Responses, None, None, Some(false)),
        (Api::Responses, Some(true), None, Some(false)),
        (Api::Responses, None, Some(false), Some(false)),
        (Api::Chat, None, None, None),
        (Api::Chat, None, Some(false), None),
        (Api::Chat, Some(true), None, None),
        (Api::Chat, None, Some(true), Some(false)),
        (Api::Chat, Some(true), Some(true), Some(false)),
        (Api::Jev, None, None, None),
        (Api::Jev, Some(true), Some(true), None),
    ];

    #[test]
    fn effort_for_matches_the_api_effort_caps_table() {
        for effort in ALL {
            for &(api, adaptive, param, want) in TABLE {
                let want = want.map(|adaptive_thinking| WireEffort {
                    effort,
                    adaptive_thinking,
                });
                assert_eq!(
                    effort_for(api, effort, &caps(adaptive, param)),
                    want,
                    "{api:?} {effort:?} adaptive={adaptive:?} param={param:?}"
                );
            }
        }
    }
}
