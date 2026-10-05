// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox self update [--version v]` (T12.2): downloads the release archive
//! for this platform from GitHub, verifies its `.sha256` checksum, and
//! replaces the running binary. Refuses to install without a matching
//! checksum; refuses Windows (rename-over-running needs a dance this does
//! not do). The download, checksum and `tar` helpers live in
//! `plugin_fetch`, shared with `cox plugin install <https-url>` (T53.2).

use std::path::PathBuf;
use std::time::Duration;

use crate::plugin_fetch::{fetch, http_client, sha256_hex, untar};

/// `pyrlyn/cox` releases carry `cox-<target>.tar.xz` built by
/// `scripts/package.sh` and published by `.github/workflows/release.yml`.
const REPO: &str = "pyrlyn/cox";

/// This platform's release target triple, if releases build it.
fn target() -> anyhow::Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-gnu"),
        (os, arch) => anyhow::bail!("no cox release for {os}/{arch}"),
    }
}

/// Whole-request limit for a release archive or its checksum.
const TIMEOUT: Duration = Duration::from_secs(120);

fn asset_base(tag: &str, target: &str) -> String {
    format!("https://github.com/{REPO}/releases/download/{tag}/cox-{target}.tar.xz")
}

/// Latest release tag via the GitHub API (public, no auth).
async fn latest_tag(client: &reqwest::Client) -> anyhow::Result<String> {
    let tag: serde_json::Value = client
        .get(format!("https://api.github.com/{REPO}/releases/latest"))
        .header("User-Agent", "cox-self-update")
        .timeout(TIMEOUT)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    tag.get("tag_name")
        .and_then(|t| t.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("latest release has no tag_name"))
}

/// Updates to `version` (a tag like `v0.1.0`) or the latest release.
pub async fn run(version: Option<String>) -> anyhow::Result<()> {
    if cfg!(windows) {
        anyhow::bail!("cox self update is not supported on Windows yet");
    }
    let target = target()?;
    let current = env!("CARGO_PKG_VERSION");
    let client = http_client()?;
    let tag = match version {
        Some(v) => v,
        None => latest_tag(&client).await?,
    };
    if tag.trim_start_matches('v') == current {
        println!("already at latest ({current})");
        return Ok(());
    }
    let base = asset_base(&tag, target);
    println!("downloading {base}");
    let archive = fetch(&client, &base).await?;
    let checksum = fetch(&client, &format!("{base}.sha256")).await?;
    let checksum = String::from_utf8_lossy(&checksum);
    let want = checksum
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow::anyhow!("checksum file is empty"))?;
    let got = sha256_hex(&archive);
    if got != want {
        anyhow::bail!("checksum mismatch for {base}: expected {want}, got {got}");
    }
    // Release archives hold the binary at the top level; unpack it.
    let exe = std::env::current_exe()?;
    let dir: PathBuf = exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("no binary dir"))?
        .into();
    let staged = dir.join("cox.update");
    unpack_cox(&archive, &staged)?;
    self_replace(&exe, &staged)?;
    println!("updated to {tag}");
    Ok(())
}

/// Extracts the `cox` binary from the `.tar.xz` archive to `dest` with the
/// system `tar` (`plugin_fetch::untar`, shared with `cox plugin install`).
fn unpack_cox(archive: &[u8], dest: &PathBuf) -> anyhow::Result<()> {
    let dir: PathBuf = dest
        .parent()
        .ok_or_else(|| anyhow::anyhow!("no binary dir"))?
        .into();
    let tmp = dir.join(".cox-update-tmp");
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp)?;
    }
    std::fs::create_dir(&tmp)?;
    let cleanup = || {
        let _ = std::fs::remove_dir_all(&tmp);
    };
    let archive_path = tmp.join("cox.tar.xz");
    if let Err(e) = (|| -> anyhow::Result<()> {
        std::fs::write(&archive_path, archive)?;
        untar(&archive_path, &tmp, &["cox"])?;
        std::fs::rename(tmp.join("cox"), dest)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    })() {
        cleanup();
        return Err(e);
    }
    cleanup();
    Ok(())
}

/// Atomically swaps the staged binary over the running one (Unix rename).
fn self_replace(exe: &PathBuf, staged: &PathBuf) -> anyhow::Result<()> {
    std::fs::rename(staged, exe)?;
    Ok(())
}
