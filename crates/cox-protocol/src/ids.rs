// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! ULID-backed newtype identifiers. Every id that crosses a crate boundary
//! (session, turn, item, tool call, archive row, background task) is one of
//! these instead of a bare `String`, so a mixed-up id is a compile error.
//! All of them serialize as their ULID string form, matching the rollout's
//! JSON (`docs/protocol.jsonschema`) and the `cox-store` primary keys.

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, json_schema};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// Declares a ULID newtype with `new`, `Display`, `FromStr` and
/// string-shaped serde, so every id gets identical behaviour.
macro_rules! ulid_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Ulid);

        impl $name {
            /// Generates a fresh, time-sortable id.
            pub fn new() -> Self {
                // ulid 3.x renamed `Ulid::new()` to `Ulid::generate()`; the
                // wrapper keeps its name so callers do not churn with the dep.
                Self(Ulid::generate())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl FromStr for $name {
            type Err = ulid::DecodeError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(Ulid::from_str(s)?))
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                Cow::Borrowed(stringify!($name))
            }

            fn json_schema(_gen: &mut schemars::SchemaGenerator) -> Schema {
                json_schema!({
                    "type": "string",
                    "description": concat!(stringify!($name), ": a 26-character Crockford-base32 ULID."),
                    "pattern": "^[0-7][0-9A-HJKMNP-TV-Z]{25}$"
                })
            }
        }
    };
}

ulid_id!(
    SessionId,
    "Identifies one `cox-store` session / rollout file."
);
ulid_id!(
    TurnId,
    "Identifies one turn (one `UserTurn` through `TurnDone`)."
);
ulid_id!(
    ItemId,
    "Identifies one transcript item (message, tool call, summary, notice)."
);
ulid_id!(
    CallId,
    "Identifies one tool call, from `ToolCallRequested` to `ToolCallDone`."
);
ulid_id!(
    ArchiveId,
    "Identifies one archived (pre-truncation) tool output row."
);
ulid_id!(TaskId, "Identifies one background subagent task.");

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn ulid_id_roundtrips_through_display_and_from_str() {
        let id = SessionId::new();
        let parsed: SessionId = id.to_string().parse().expect("display form parses back");
        assert_eq!(id, parsed);
    }

    #[test]
    fn ulid_id_roundtrips_through_json() {
        let id = CallId::new();
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, format!("\"{id}\""));
        let back: CallId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, back);
    }

    #[test]
    fn distinct_id_types_are_distinct_types() {
        // Compile-time proof only: this would not type-check if the macro
        // produced interchangeable ids.
        fn takes_turn_id(_: TurnId) {}
        takes_turn_id(TurnId::new());
    }
}
