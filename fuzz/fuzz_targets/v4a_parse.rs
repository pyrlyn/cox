// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Fuzz target for the V4A grammar (plan.md T3.5 "Done when", run in
//! T12.4). Attacks `cox_tools::v4a::parse` (re-exported from `cox-patch`,
//! T32.6) from the opposite side of the round-trip property in
//! `cox-patch/src/parse.rs`: that test feeds it patches it built itself,
//! this feeds it arbitrary bytes.
//!
//! Two claims, both of which a panic here would disprove:
//!   1. `parse` never panics — every malformed input is a `ToolError`, not
//!      an index-out-of-bounds or a UTF-8 boundary slice.
//!   2. Parsing is idempotent through printing: anything `parse` accepts,
//!      `Display` prints back into something `parse` accepts identically.
//!      A patch that parsed but printed differently would silently change
//!      files whenever cox echoed one back.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(patch) = cox_tools::v4a::parse(text) {
        let printed = patch.to_string();
        let reparsed = cox_tools::v4a::parse(&printed).expect("printed patch must re-parse");
        assert_eq!(reparsed, patch, "print/parse round-trip changed the patch");
    }
});
