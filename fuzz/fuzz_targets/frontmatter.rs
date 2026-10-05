// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Fuzz target for extension frontmatter (T12.4): `split` and `parse` must
//! never panic — a broken skill/command/agent file is a `Notice`, never a
//! crash (D14 fail-open).

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = cox_ext::frontmatter::split(text);
    let _ = cox_ext::frontmatter::parse::<serde_json::Value>(text);
});
