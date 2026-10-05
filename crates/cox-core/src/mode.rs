// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Mode presets over permission mode and main tier (P42, A73): the
//! top-session counterpart of `subagent::PRESETS`. Separate from `session`
//! so the preset table and the one narrowing rule have a single home that
//! `/mode`, session build and T45.2 all read. A mode never touches tools:
//! "no writes" comes from `PermissionMode::Plan` through the engine, so the
//! cached system[0..2] stay byte-stable across a switch (invariant 1).

use cox_protocol::types::{Mode, PermissionMode, Tier};

/// What one mode changes; `None` leaves the session's value as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModePreset {
    /// The mode this preset implements.
    pub mode: Mode,
    /// The permission mode it narrows to; never widens (`apply`).
    pub permission: Option<PermissionMode>,
    /// The tier main turns run on while the mode stands.
    pub main_tier: Option<Tier>,
}

/// The configured permission mode and main tier, unchanged.
pub const EDITOR: ModePreset = ModePreset {
    mode: Mode::Editor,
    permission: None,
    main_tier: None,
};

/// Read-only tools only, main turns on the think tier (still confirmed,
/// invariant 9).
pub const ARCHITECT: ModePreset = ModePreset {
    mode: Mode::Architect,
    permission: Some(PermissionMode::Plan),
    main_tier: Some(Tier::Think),
};

/// The preset for `mode`.
pub fn preset(mode: Mode) -> ModePreset {
    match mode {
        Mode::Editor => EDITOR,
        Mode::Architect => ARCHITECT,
    }
}

/// The permission mode `preset` leaves in force over `configured`: the
/// narrower of the two, so a mode never widens `permissions.mode`.
pub fn apply(preset: ModePreset, configured: PermissionMode) -> PermissionMode {
    preset
        .permission
        .map_or(configured, |p| cox_permission::narrower(configured, p))
}

/// `/mode`'s argument, as `core.mode` spells it.
pub fn parse(arg: &str) -> Option<Mode> {
    match arg.trim() {
        "architect" => Some(Mode::Architect),
        "editor" => Some(Mode::Editor),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn architect_never_widens_any_configured_mode() {
        for configured in [
            PermissionMode::Plan,
            PermissionMode::Default,
            PermissionMode::Auto,
            PermissionMode::Bypass,
        ] {
            assert_eq!(apply(ARCHITECT, configured), PermissionMode::Plan);
            assert_eq!(apply(EDITOR, configured), configured);
        }
    }

    #[test]
    fn parse_takes_only_the_two_mode_names() {
        assert_eq!(parse("architect"), Some(Mode::Architect));
        assert_eq!(parse(" editor "), Some(Mode::Editor));
        assert_eq!(parse("plan"), None);
        assert_eq!(preset(Mode::Architect), ARCHITECT);
    }
}
