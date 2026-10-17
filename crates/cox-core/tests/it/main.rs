//! One integration-test binary for cox-core: every module below was its own
//! `tests/*.rs` binary, so nextest linked 24 of them (T61.5.1). A module keeps its
//! old name, so `-E 'test(/^compact::/)'` selects what `--test compact` did.

// Shared with cox-app's tests, which include it by path; it stays outside `it/`.
#[path = "../common/mod.rs"]
mod common;

mod allow_for_session;
mod approve_writes;
mod bash_tasks;
mod budget;
mod chat_wire;
mod checkpoint;
mod compact;
mod context;
mod dedup;
mod hooks;
mod memory_extract;
mod microcompact;
mod permission;
mod policy_matrix;
mod question;
mod resume;
mod rewind;
mod router;
mod same_path;
mod subagent;
mod telemetry;
mod turn;
mod user_agent;
mod user_shell;
