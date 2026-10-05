// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Fetching what `cox` installs from outside the machine (T53.2, PL§1): the
//! HTTP client, download and SHA-256 helpers `cox self update`, `cox plugin
//! install <https-url>` and `cox voice model download` share, the `tar`
//! shell-out the first two unpack with, and the staging directory a
//! downloaded or cloned (`git+<url>`, T53.3) plugin lands in before the
//! local install path (`plugin_cmd`) takes over. Separate from `plugin_cmd`
//! so the self-update path reuses one fetch without the plugin host, and so
//! every rule about an untrusted downloaded tree sits in one place: the
//! hash is checked before a byte is unpacked, no entry may be a link or
//! leave staging, nothing in the tree runs, and staging is removed on every
//! exit.

// The slim build (no `plugins` feature) uses only the self-update helpers.
#![cfg_attr(not(feature = "plugins"), allow(dead_code))]

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail};
use cox_sanitize::sanitize;
use sha2::{Digest, Sha256};

const USER_AGENT: &str = concat!("cox/", env!("CARGO_PKG_VERSION"));

/// A staging entry older than this was left by an install that crashed:
/// nothing else writes under `.staging/`, and no install takes a day.
const STALE: Duration = Duration::from_secs(24 * 60 * 60);

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The client cox's own downloads go through: a `cox/<version>`
/// User-Agent and nothing else about the user. A whole-request limit is
/// each caller's, since a voice model is hundreds of MB; a stalled read
/// still fails.
pub(crate) fn http_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(60))
        .build()
}

/// SHA-256 of everything `reader` yields, as lowercase hex; read in
/// chunks so a voice model is never held in memory whole (T54.5).
#[cfg_attr(not(feature = "voice"), allow(dead_code))]
pub(crate) fn sha256_read(mut reader: impl Read) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    let mut chunk = vec![0; 1 << 16];
    loop {
        let n = reader.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        hasher.update(&chunk[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Downloads `url` fully.
pub async fn fetch(client: &reqwest::Client, url: &str) -> anyhow::Result<Vec<u8>> {
    Ok(client
        .get(url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?
        .to_vec())
}

/// `tar -xf <archive> -C <into> [members…]` with the system `tar`: BSD and
/// GNU tar both detect the compression of a file they read, so one call
/// unpacks a release's `.tar.xz` and a plugin's `.tar.gz` without a
/// C-linked extraction dependency.
pub fn untar(archive: &Path, into: &Path, members: &[&str]) -> anyhow::Result<()> {
    let status = Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .args(members)
        .status()
        .map_err(|e| anyhow!("tar not found: {e}"))?;
    if !status.success() {
        bail!("tar failed: {status}");
    }
    Ok(())
}

/// One install's private directory under `<cox_home>/plugins/.staging/`
/// (PL§1), removed when dropped — on success, on a refusal and on a panic
/// alike; a crash's leftover is swept by the next install.
pub struct Staging {
    dir: PathBuf,
}

impl Staging {
    pub fn new(cox_home: &Path) -> anyhow::Result<Self> {
        let root = cox_home.join("plugins").join(".staging");
        fs::create_dir_all(&root)?;
        sweep(&root);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir = root.join(format!("{}-{nanos}", std::process::id()));
        // `create_dir`, not `create_dir_all`: two installs never share one.
        fs::create_dir(&dir)?;
        Ok(Self { dir })
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
        // Only succeeds once no other install is staging.
        if let Some(root) = self.dir.parent() {
            let _ = fs::remove_dir(root);
        }
    }
}

fn sweep(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > STALE);
        if stale {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// PL§1: only `https://` is fetched; `http://`, `file://` and every other
/// scheme are refused before a byte is fetched.
pub fn https_url(url: &str) -> anyhow::Result<()> {
    let parsed =
        reqwest::Url::parse(url).map_err(|e| anyhow!("{} is not a URL: {e}", sanitize(url)))?;
    if parsed.scheme() != "https" {
        bail!(
            "refused {}: a plugin URL must be https://",
            sanitize(parsed.scheme())
        );
    }
    Ok(())
}

/// `--sha256` is required with a URL (PL§1) and must be a whole digest.
pub fn sha256_arg(hex: Option<&str>) -> anyhow::Result<String> {
    let hex = hex
        .ok_or_else(|| anyhow!("--sha256 <hex> is required to install from a URL"))?
        .to_ascii_lowercase();
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("--sha256 must be 64 hex digits");
    }
    Ok(hex)
}

/// Downloads `url` into `staging`, refuses it unless its SHA-256 is
/// `sha256` (before anything is unpacked), unpacks it and returns the
/// package root. The archive itself is deleted, so only the tree reaches
/// the install. Checks no scheme: `https_url` is the caller's guard, which
/// is what lets a test reach a plain-http mock server through here.
pub fn fetch_archive(staging: &Staging, url: &str, sha256: &str) -> anyhow::Result<PathBuf> {
    let bytes = download(url)?;
    let got = sha256_hex(&bytes);
    if got != sha256 {
        bail!(
            "sha256 mismatch for {}: expected {sha256}, got {got}; nothing was unpacked",
            sanitize(url)
        );
    }
    let archive = staging.path().join("package.tar");
    fs::write(&archive, &bytes)?;
    check_entries(&archive)?;
    let tree = staging.path().join("tree");
    fs::create_dir(&tree)?;
    untar(&archive, &tree, &[])?;
    fs::remove_file(&archive)?;
    refuse_links(&tree)?;
    package_root(&tree)
}

/// Clones `rev` of the git repository at `url` into `staging` and returns
/// the package root (`path` inside the clone, `.git` removed) and the
/// commit it resolved (PL§1). `rev` must be a tag or a full commit hash:
/// a branch is refused, so `update` never follows a moving target.
pub fn clone_git(
    staging: &Staging,
    url: &str,
    rev: &str,
    path: &str,
) -> anyhow::Result<(PathBuf, String)> {
    git_url(url)?;
    if rev.is_empty()
        || rev.starts_with('-')
        || rev.contains("..")
        || !rev
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._/+-".contains(&b))
    {
        bail!("--rev {} is not a tag or a commit hash", sanitize(rev));
    }
    let sub = Path::new(path);
    if !sub
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
    {
        bail!("--path {} must stay inside the clone", sanitize(path));
    }
    let top = staging.path();
    let clone = top.join("clone");
    let clone_arg = clone.to_string_lossy();
    let refs = git(top, &["ls-remote", "--tags", "--heads", "--", url])?;
    let names = |kind: &str| {
        let want = format!("refs/{kind}/{rev}");
        refs.lines()
            .any(|l| l.split('\t').nth(1) == Some(want.as_str()))
    };
    let is_commit = matches!(rev.len(), 40 | 64) && rev.bytes().all(|b| b.is_ascii_hexdigit());
    match (names("tags"), names("heads")) {
        (_, true) => bail!(
            "--rev {} names a branch; install from a tag or a commit",
            sanitize(rev)
        ),
        (true, false) => {
            git(
                top,
                &[
                    "clone",
                    "--quiet",
                    "--depth",
                    "1",
                    "--no-recurse-submodules",
                    "--branch",
                    rev,
                    "--",
                    url,
                    &clone_arg,
                ],
            )?;
        }
        (false, false) if is_commit => {
            git(top, &["init", "--quiet", &clone_arg])?;
            git(
                &clone,
                &[
                    "fetch",
                    "--quiet",
                    "--depth",
                    "1",
                    "--no-recurse-submodules",
                    "--",
                    url,
                    rev,
                ],
            )?;
            git(&clone, &["checkout", "--quiet", "--detach", "FETCH_HEAD"])?;
        }
        (false, false) => bail!(
            "--rev {} is neither a tag of that repository nor a full commit hash",
            sanitize(rev)
        ),
    }
    let commit = git(&clone, &["rev-parse", "HEAD"])?.trim().to_string();
    fs::remove_dir_all(clone.join(".git"))?;
    // Each step of `path` must be a real directory: a symlinked one could
    // point anywhere, whatever its name says.
    let mut root = clone;
    for part in sub.components() {
        root.push(part);
        if !fs::symlink_metadata(&root).is_ok_and(|m| m.is_dir()) {
            bail!(
                "--path {} is not a directory inside the clone",
                sanitize(path)
            );
        }
    }
    refuse_links(&root)?;
    Ok((root, commit))
}

/// `https://`, `ssh://` and `file://` only: plain `http://` and `git://`
/// are unauthenticated, and a `<transport>::<address>` helper (`ext::` runs
/// a command) parses as its own scheme, so it never reaches `git`.
fn git_url(url: &str) -> anyhow::Result<()> {
    let scheme = reqwest::Url::parse(url)
        .map(|u| u.scheme().to_string())
        .map_err(|e| anyhow!("git+{} is not a URL: {e}", sanitize(url)))?;
    if !matches!(scheme.as_str(), "https" | "ssh" | "file") {
        bail!(
            "refused git+{}: the URL must be https://, ssh:// or file://",
            sanitize(url)
        );
    }
    Ok(())
}

/// One `git` run, shelled to the way `cox_tools::git` does (A13): `git` on
/// `PATH`, never linked in. No prompt (`GIT_TERMINAL_PROMPT=0`, no stdin),
/// so a private repository fails instead of waiting, and
/// `GIT_CEILING_DIRECTORIES` stops git from finding a repository above
/// staging and reading its config.
fn git(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    let ceiling = dir
        .ancestors()
        .find(|a| a.file_name().is_some_and(|n| n == ".staging"))
        .unwrap_or(dir);
    let out = Command::new("git")
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CEILING_DIRECTORIES", ceiling)
        .args(["-c", "core.fsmonitor=false"])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| anyhow!("git not found: {e}"))?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.first().copied().unwrap_or_default(),
            sanitize(String::from_utf8_lossy(&out.stderr).trim())
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Runs on its own thread with its own runtime: every caller is sync, and
/// `/plugin update` arrives from inside the TUI's runtime, where a nested
/// `block_on` would panic.
fn download(url: &str) -> anyhow::Result<Vec<u8>> {
    std::thread::scope(|s| {
        s.spawn(|| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            rt.block_on(async {
                let client = reqwest::Client::builder()
                    .timeout(Duration::from_secs(120))
                    .redirect(https_redirects())
                    .build()?;
                fetch(&client, url).await
            })
        })
        .join()
        .map_err(|_| anyhow!("the download thread panicked"))?
    })
}

/// A redirect may not step down from https, or the "refused before a byte
/// is fetched" rule would hold only for the first hop.
fn https_redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 10 {
            attempt.error("too many redirects")
        } else if attempt.url().scheme() != "https" {
            attempt.error("a redirect away from https:// is refused")
        } else {
            attempt.follow()
        }
    })
}

/// Reads the archive's table of contents before extracting: every name
/// stays inside staging, and every entry is a plain file or directory —
/// a symlink or hard link is refused before `tar` could create it.
fn check_entries(archive: &Path) -> anyhow::Result<()> {
    for name in tar_list(archive, "-tf")?.lines() {
        let escapes = Path::new(name).components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        });
        if escapes {
            bail!(
                "refused: archive entry {} leaves the staging directory",
                sanitize(name)
            );
        }
    }
    // `tar -tv` starts each line with the entry's type: `-` file, `d`
    // directory, `l` symlink, `h` hard link (GNU and BSD alike).
    for line in tar_list(archive, "-tvf")?.lines() {
        if !line.is_empty() && !line.starts_with(['-', 'd']) {
            bail!(
                "refused: archive entry is not a plain file or directory: {}",
                sanitize(line)
            );
        }
    }
    Ok(())
}

fn tar_list(archive: &Path, flags: &str) -> anyhow::Result<String> {
    let out = Command::new("tar")
        .arg(flags)
        .arg(archive)
        .output()
        .map_err(|e| anyhow!("tar not found: {e}"))?;
    if !out.status.success() {
        bail!(
            "tar cannot read the archive: {}",
            sanitize(String::from_utf8_lossy(&out.stderr).trim())
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Refuses any symlink or special file under `dir`, without following one
/// (`file_type` is `lstat`): the last guard, after `tar` and `git` wrote.
pub fn refuse_links(dir: &Path) -> anyhow::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            refuse_links(&entry.path())?;
        } else if !ty.is_file() {
            bail!(
                "refused: {} is a link or a special file",
                sanitize(&entry.file_name().to_string_lossy())
            );
        }
    }
    Ok(())
}

/// The package is the tree itself, or its one top-level directory — the
/// usual `<name>-<version>/` layout of a release archive.
fn package_root(tree: &Path) -> anyhow::Result<PathBuf> {
    if tree.join("plugin.toml").is_file() {
        return Ok(tree.to_path_buf());
    }
    let mut entries = fs::read_dir(tree)?.filter_map(Result::ok);
    match (entries.next(), entries.next()) {
        (Some(only), None) if only.path().join("plugin.toml").is_file() => Ok(only.path()),
        _ => bail!("the archive has no plugin.toml at its top level or in its one top directory"),
    }
}
