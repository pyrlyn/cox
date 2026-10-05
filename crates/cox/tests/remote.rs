// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! T52.20: a remote workspace end to end. `RemoteWorkspace` starts a fake
//! `ssh` that records what it was given and then runs the command after the
//! host — the built `cox app-server --stdio` — so the session, the wire and
//! the patch stream are the shipped ones, only the network is missing.

// why: env::set_var/remove_var are unsafe in edition 2024 (per-process tests).
#![allow(unsafe_code)]
#![cfg(feature = "app-server")]

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use cox_app::app::Host;
use cox_app::remote::RemoteWorkspace;
use cox_app::{BlockKind, InboxItem, Intent, TimelinePatch};

/// One plain reply.
const REPLY: &str = r#"
[[turn]]
text = "Hello from the remote."
"#;

/// The local app's host: nothing to show in a test.
struct Quiet;

impl Host for Quiet {
    fn notify(&self, _: InboxItem, _: u32) {}
    fn badge(&self, _: u32) {}
    fn open_url(&self, _: &str) {}
    fn secret(&self, _: &str) -> Option<String> {
        None
    }
}

fn ends_turn(patch: &TimelinePatch) -> bool {
    matches!(patch, TimelinePatch::Upsert { block, .. }
        if matches!(block.kind, BlockKind::TurnMeta { stop: Some(_), .. }))
}

/// The fake ssh: writes its arguments and environment next to itself, skips
/// the options, `--`, the host and the remote `cox`, and runs the built
/// binary. ssh scrubbed its environment, so the scratch home and the
/// scripted provider are written into the script, as the remote machine's
/// own settings would be.
fn fake_ssh(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("ssh");
    let script = format!(
        r#"#!/bin/sh
printf '%s\n' "$@" > "{dir}/ssh-args"
env > "{dir}/ssh-env"
while [ "$1" != "--" ]; do shift; done
shift 3
export COX_HOME="{dir}/remote/.cox" COX_PROVIDER=scripted COX_SCENARIO="{dir}/scenario.toml"
export COX_KEYRING=off
exec "{cox}" "$@"
"#,
        dir = dir.display(),
        cox = env!("CARGO_BIN_EXE_cox"),
    );
    std::fs::write(&path, script).expect("write the fake ssh");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

#[tokio::test]
async fn remote_session_streams_through_a_fake_ssh() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("project")).expect("project");
    std::fs::create_dir_all(root.join("remote")).expect("remote home");
    std::fs::write(root.join("scenario.toml"), REPLY).expect("scenario");
    // SAFETY: first thing in this test's own process (nextest).
    unsafe {
        std::env::set_var("HOME", root.join("remote"));
        std::env::set_var("ANTHROPIC_API_KEY", "sk-must-stay-local");
    }
    let ssh = fake_ssh(root);

    let workspace = RemoteWorkspace::connect_with(&ssh, "devbox", Arc::new(Quiet))
        .await
        .expect("connect");
    let session = workspace
        .open(root.join("project"), None, "base16-ocean.dark".into())
        .await
        .expect("open");
    let child = session
        .send(Intent::Send {
            text: "hi".into(),
            attachments: vec![],
            confirm_think: false,
        })
        .await
        .expect("send");
    assert!(child.is_none());
    let ended = tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(batch) = session.next_patches().await {
            if batch.iter().any(ends_turn) {
                return true;
            }
        }
        false
    })
    .await;
    assert_eq!(ended, Ok(true), "the remote turn's patches did not arrive");
    assert!(workspace.is_connected());

    let args = std::fs::read_to_string(root.join("ssh-args")).expect("ssh args");
    let args: Vec<&str> = args.lines().collect();
    assert!(args.contains(&"ForwardAgent=no"), "{args:?}");
    assert!(args.contains(&"BatchMode=yes"), "{args:?}");
    let env = std::fs::read_to_string(root.join("ssh-env")).expect("ssh env");
    assert!(
        !env.contains("ANTHROPIC_API_KEY"),
        "a local key reached ssh"
    );
}
