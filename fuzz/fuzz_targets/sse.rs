// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Fuzz target for the Anthropic SSE parser (T12.4): arbitrary bytes must
//! never panic `parse_sse_str` — malformed frames are skipped, truncated
//! tails dropped, bad JSON a `Parse` error.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = cox_provider::sse::parse_sse_str(text);
});
