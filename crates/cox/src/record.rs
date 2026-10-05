// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox record <name>`: write a provider cassette for `Replay` (plan.md T1.5).
//! Live session capture waits on T2.1; this command hashes a one-shot
//! request from `-p` and stores an `--sse` body (or a fixture path).

use std::path::PathBuf;

use anyhow::Context;
use cox_protocol::types::{Content, Effort, Job, Message, ModelId, Request, Role, Thinking, Tier};
use cox_provider::replay::write_cassette;

use crate::cli::{Cli, RecordArgs};

/// Writes `cassettes/<name>/` under `home` (or `COX_HOME`).
pub fn run(cli: &Cli, args: &RecordArgs) -> anyhow::Result<()> {
    let home = cli
        .home
        .clone()
        .or_else(|| std::env::var_os("COX_HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = home.join("cassettes").join(&args.name);
    let prompt = args
        .prompt
        .as_deref()
        .context("cox record requires -p <prompt>")?;
    let sse_path = args
        .sse
        .as_ref()
        .context("cox record requires --sse <file> until a live session is wired")?;
    let sse = std::fs::read_to_string(sse_path)
        .with_context(|| format!("read {}", sse_path.display()))?;
    let req = Request {
        tier: Tier::Code,
        job: Job::Main,
        model: ModelId(
            cli.model
                .clone()
                .unwrap_or_else(|| "claude-sonnet-5".into()),
        ),
        system: vec![],
        tools: vec![],
        messages: vec![Message {
            role: Role::User,
            content: vec![Content::Text {
                text: prompt.to_string(),
            }],
        }],
        effort: Effort::High,
        max_tokens: 1024,
        thinking: Thinking::Off,
        cache_breakpoints: vec![],
        stop_sequences: vec![],
    };
    let hash = write_cassette(&dir, &req, &sse, args.redact)
        .map_err(|e| anyhow::anyhow!("write cassette: {e}"))?;
    println!("{} {hash}", dir.display());
    Ok(())
}
