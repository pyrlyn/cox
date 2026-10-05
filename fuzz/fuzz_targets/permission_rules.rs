// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Fuzz target for permission rules (T12.4): compiling arbitrary rule
//! strings and deciding synthetic calls must never panic. A malformed rule
//! is a config error; an undecidable call asks or denies — both are values,
//! never panics. (A wrong *verdict* is a logic bug fuzzing cannot see; the
//! table tests in `cox-core` own that.)

#![no_main]

use std::path::Path;

use cox_core::Engine;
use cox_protocol::types::{
    ApprovalPolicy, PermissionMode, Risk, SandboxMode, ToolCall,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let mut lines = text.lines();
    let allow: Vec<String> = lines.by_ref().take(4).map(str::to_string).collect();
    let ask: Vec<String> = lines.by_ref().take(4).map(str::to_string).collect();
    let deny: Vec<String> = lines.by_ref().take(4).map(str::to_string).collect();
    let mut cfg = cox_protocol::config::PermissionsConfig::default();
    cfg.allow = allow;
    cfg.ask = ask;
    cfg.deny = deny;
    let Ok(engine) = Engine::compile(&cfg, None, Path::new("/tmp")) else {
        return;
    };
    let call = ToolCall {
        id: cox_protocol::CallId::new(),
        name: "read".to_string(),
        input: serde_json::json!({}),
        risk: Risk::ReadOnly,
        subject: "/tmp/x".to_string(),
    };
    for mode in [
        PermissionMode::Default,
        PermissionMode::Plan,
        PermissionMode::Auto,
        PermissionMode::Bypass,
    ] {
        for policy in [
            ApprovalPolicy::Untrusted,
            ApprovalPolicy::OnRequest,
            ApprovalPolicy::OnFailure,
            ApprovalPolicy::Never,
        ] {
            let _ = engine.decide(
                &call,
                mode,
                policy,
                SandboxMode::WorkspaceWrite,
                &[],
            );
        }
    }
});
