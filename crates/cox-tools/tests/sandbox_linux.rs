// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T4.2: `bash` under bubblewrap or Landlock — the same guarantees as the
//! macOS file, through the real tool. CI runs this twice, with and without
//! `bwrap` on PATH, and `COX_EXPECT_SANDBOX` pins which backend each job
//! must have picked so a silent fallback cannot pass.

#![cfg(target_os = "linux")]

mod common;

use std::path::Path;

use cox_protocol::{LinuxBackend, SandboxMode, Tool, ToolOutput};
use cox_tools::bash::BashTool;
use cox_tools::sandbox::{Backend, backend};
use serde_json::json;
use tokio_util::sync::CancellationToken;

fn active() -> Backend {
    backend(LinuxBackend::Auto).expect("CI hosts provide bwrap or a Landlock kernel")
}

async fn bash(root: &Path, mode: SandboxMode, network: bool, command: &str) -> ToolOutput {
    let mut policy = common::policy(mode);
    policy.network = network;
    let (cx, _rx) = common::cx(root.to_path_buf(), policy, CancellationToken::new());
    BashTool
        .call(json!({"command": command}), &cx)
        .await
        .expect("bash runs")
}

#[test]
fn sandbox_linux_picks_the_backend_the_job_expects() {
    let got = active().name();
    if let Ok(want) = std::env::var("COX_EXPECT_SANDBOX") {
        assert_eq!(got, want);
    }
}

#[tokio::test]
async fn sandbox_linux_workspace_write_allows_writes_inside_the_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = bash(
        dir.path(),
        SandboxMode::WorkspaceWrite,
        false,
        "echo in > inside",
    )
    .await;
    assert!(!out.is_error, "{}", out.text);
    let got = std::fs::read_to_string(dir.path().join("inside")).expect("written");
    assert_eq!(got, "in\n");
}

#[tokio::test]
async fn sandbox_linux_denies_writes_outside_the_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = std::env::var("HOME").expect("HOME");
    let outside = format!("{home}/.cox-sandbox-escape-{}", std::process::id());
    let out = bash(
        dir.path(),
        SandboxMode::WorkspaceWrite,
        false,
        &format!("echo x > '{outside}'"),
    )
    .await;
    let leaked = Path::new(&outside).exists();
    let _ = std::fs::remove_file(&outside);
    assert!(!leaked, "the sandbox let a write escape to {outside}");
    assert!(out.is_error, "{}", out.text);
}

#[tokio::test]
async fn sandbox_linux_keeps_git_read_only_inside_the_root() {
    if active() != Backend::Bwrap {
        // Landlock rules only grant; a read-only subpath inside a writable
        // root needs bind mounts (done.md T4.2).
        eprintln!("skipped: {} cannot carve out .git", active().name());
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let head = dir.path().join(".git").join("HEAD");
    std::fs::create_dir_all(dir.path().join(".git")).expect("mkdir");
    std::fs::write(&head, "ref: refs/heads/main\n").expect("seed");
    let out = bash(
        dir.path(),
        SandboxMode::WorkspaceWrite,
        false,
        "echo broken > .git/HEAD",
    )
    .await;
    assert!(out.is_error, "{}", out.text);
    let got = std::fs::read_to_string(&head).expect("still there");
    assert_eq!(got, "ref: refs/heads/main\n");
}

#[tokio::test]
async fn sandbox_linux_read_only_denies_writes_inside_the_root() {
    // Under bwrap an unbound root under /tmp would not even exist, so the
    // root lives under $HOME, which the read-only `/` bind carries.
    let home = std::env::var("HOME").expect("HOME");
    let root = Path::new(&home).join(format!(".cox-sandbox-ro-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("mkdir");
    let out = bash(&root, SandboxMode::ReadOnly, false, "echo in > inside").await;
    let leaked = root.join("inside").exists();
    let _ = std::fs::remove_dir_all(&root);
    assert!(!leaked, "read-only let a write through");
    assert!(out.is_error, "{}", out.text);
}

#[tokio::test]
async fn sandbox_linux_blocks_the_network_unless_allowed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = bash(
        dir.path(),
        SandboxMode::WorkspaceWrite,
        false,
        "curl -sS --max-time 3 https://example.com",
    )
    .await;
    assert!(out.is_error, "{}", out.text);
    assert!(out.text.contains("curl: ("), "{}", out.text);
}
