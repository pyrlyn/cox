// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Pre-images for `/rewind` (T26.1): what a file held before a tool call
//! changed it, including changes a shell command made that no tool named.
//! Lives here rather than in `cox-core` because it reads files and runs
//! `git` — the loop only decides when to ask (`cox_core::checkpoint`).
//!
//! The workspace snapshot is a git tree written from a *private* repository
//! under `<home>/checkpoints/<root hash>` whose work tree is the workspace
//! root: `git add -A` into that repository's own index (never the user's)
//! then `write-tree`. The private index doubles as the stat cache, so a
//! warm snapshot is one stat pass over the tree; `.gitignore` keeps build
//! output out; and `GIT_ALTERNATE_OBJECT_DIRECTORIES` pointed at the real
//! repository means unchanged blobs are never copied. A root that is not a
//! git repository works the same way, it just has no alternate.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use cox_protocol::{Before, Change, Checkpointer, PreImage, Snapshot, ToolError};
use tokio::process::Command;

use crate::path::confine;

/// A pre-image over this size is recorded as `Before::TooLarge` instead of
/// archived: `/rewind` will say so rather than restore it.
pub const PREIMAGE_LIMIT: usize = 8 * 1024 * 1024;

/// The `Checkpointer` the binary installs (`Session::set_checkpointer`).
pub struct GitCheckpointer {
    /// `COX_HOME`; private repositories live under `<home>/checkpoints`.
    home: PathBuf,
}

impl GitCheckpointer {
    /// Snapshots go under `<home>/checkpoints`.
    pub fn new(home: PathBuf) -> Self {
        Self { home }
    }

    /// One private repository per root. `DefaultHasher` is not stable
    /// across Rust releases; a changed name only costs one cold snapshot.
    fn git_dir(&self, root: &Path) -> PathBuf {
        let mut h = DefaultHasher::new();
        root.hash(&mut h);
        self.home
            .join("checkpoints")
            .join(format!("{:016x}", h.finish()))
    }

    /// Runs `git` against the private repository with `root` as the work
    /// tree, creating the repository on first use.
    async fn git(&self, root: &Path, args: &[&str]) -> Result<Vec<u8>, ToolError> {
        let dir = self.git_dir(root);
        if !dir.join("HEAD").exists() {
            tokio::fs::create_dir_all(&dir)
                .await
                .map_err(|_| ToolError::Io)?;
            run(Command::new("git").args(["init", "-q", "--bare"]).arg(&dir)).await?;
        }
        let mut cmd = Command::new("git");
        cmd.current_dir(root)
            .arg("--git-dir")
            .arg(&dir)
            .arg("--work-tree")
            .arg(root)
            // The user's environment must not redirect the private repo.
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .args(["-c", "core.bare=false", "-c", "core.autocrlf=false"])
            .args(["-c", "core.fsmonitor=false", "-c", "core.quotePath=false"]);
        if let Some(objects) = real_objects(root).await {
            cmd.env("GIT_ALTERNATE_OBJECT_DIRECTORIES", objects);
        }
        run(cmd.args(args)).await
    }
}

/// The workspace's own object directory, when the root is a git repository
/// (a linked worktree reports its main repository's).
async fn real_objects(root: &Path) -> Option<PathBuf> {
    let out = Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "--git-path", "objects"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let rel = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let path = root.join(rel);
    path.is_dir().then_some(path)
}

async fn run(cmd: &mut Command) -> Result<Vec<u8>, ToolError> {
    let out = cmd.output().await.map_err(|_| ToolError::Io)?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(ToolError::Io)
    }
}

/// `diff-tree -z --name-status` output: `<status>\0<path>\0…`.
fn parse_name_status(out: &[u8]) -> Vec<(u8, String)> {
    let mut fields = out.split(|b| *b == 0).filter(|f| !f.is_empty());
    let mut rows = Vec::new();
    while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
        let Some(&code) = status.first() else {
            continue;
        };
        rows.push((code, String::from_utf8_lossy(path).into_owned()));
    }
    rows
}

fn sized(bytes: Vec<u8>) -> Before {
    if bytes.len() > PREIMAGE_LIMIT {
        Before::TooLarge
    } else {
        Before::Bytes(bytes)
    }
}

#[async_trait]
impl Checkpointer for GitCheckpointer {
    async fn preimages(&self, roots: &[PathBuf], cwd: &Path, paths: &[String]) -> Vec<PreImage> {
        let mut out = Vec::new();
        for p in paths {
            let Ok(path) = confine(roots, cwd, p) else {
                continue;
            };
            let before = match tokio::fs::read(&path).await {
                Ok(bytes) => sized(bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Before::Absent,
                // A directory or an unreadable file: the tool will fail on
                // it too, and there is nothing to restore.
                Err(_) => continue,
            };
            out.push(PreImage { path, before });
        }
        out
    }

    async fn snapshot(&self, roots: &[PathBuf]) -> Result<Snapshot, ToolError> {
        let mut trees = Vec::with_capacity(roots.len());
        for root in roots {
            self.git(root, &["add", "-A", "--ignore-errors", "--", "."])
                .await?;
            let tree = self.git(root, &["write-tree"]).await?;
            let tree = String::from_utf8_lossy(&tree).trim().to_string();
            trees.push((root.clone(), tree));
        }
        Ok(Snapshot { trees })
    }

    async fn changes(&self, before: &Snapshot, after: &Snapshot) -> Result<Vec<Change>, ToolError> {
        let mut out = Vec::new();
        for ((root, old), (_, new)) in before.trees.iter().zip(&after.trees) {
            if old == new {
                continue;
            }
            let raw = self
                .git(root, &["diff-tree", "-r", "-z", "--name-status", old, new])
                .await?;
            for (code, rel) in parse_name_status(&raw) {
                let path = root.join(&rel);
                let deleted = code == b'D';
                let before = if code == b'A' {
                    Before::Absent
                } else {
                    let spec = format!("{old}:{rel}");
                    sized(self.git(root, &["cat-file", "blob", &spec]).await?)
                };
                out.push(Change {
                    path,
                    deleted,
                    before,
                });
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    async fn restore(
        &self,
        roots: &[PathBuf],
        cwd: &Path,
        path: &Path,
        bytes: Option<&[u8]>,
    ) -> Result<(), ToolError> {
        let path = confine(roots, cwd, &path.to_string_lossy())?;
        match bytes {
            Some(bytes) => crate::write::atomic_write(&path, bytes),
            None => match tokio::fs::remove_file(&path).await {
                Ok(()) => Ok(()),
                // Already gone is the state we want.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(_) => Err(ToolError::Io),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// Skips the test when there is no usable `git` (the parser tests
    /// below still run).
    async fn have_git() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .await
            .is_ok_and(|o| o.status.success())
    }

    fn workspace() -> (tempfile::TempDir, PathBuf, GitCheckpointer) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("ws");
        fs::create_dir_all(root.join("src")).expect("mkdir");
        // `confine` canonicalizes (macOS: `/var` → `/private/var`).
        let root = root.canonicalize().expect("canonicalize");
        fs::write(root.join("src/a.rs"), "one\n").expect("write");
        fs::write(root.join(".gitignore"), "target/\n").expect("write");
        let cp = GitCheckpointer::new(dir.path().join("home"));
        (dir, root, cp)
    }

    #[test]
    fn name_status_parses_nul_separated_rows() {
        let rows = parse_name_status(b"M\0src/a.rs\0D\0gone.txt\0A\0new file.txt\0");
        assert_eq!(
            rows,
            vec![
                (b'M', "src/a.rs".to_string()),
                (b'D', "gone.txt".to_string()),
                (b'A', "new file.txt".to_string()),
            ]
        );
        assert!(parse_name_status(b"").is_empty());
    }

    #[tokio::test]
    async fn snapshot_diff_reports_modified_deleted_and_created_with_preimages() {
        if !have_git().await {
            return;
        }
        let (_dir, root, cp) = workspace();
        fs::write(root.join("gone.txt"), "bye\n").expect("write");
        let roots = vec![root.clone()];
        let before = cp.snapshot(&roots).await.expect("snapshot");
        assert_eq!(cp.changes(&before, &before).await.expect("same"), vec![]);

        fs::write(root.join("src/a.rs"), "two\n").expect("edit");
        fs::remove_file(root.join("gone.txt")).expect("rm");
        fs::write(root.join("made.txt"), "hi\n").expect("create");
        fs::create_dir_all(root.join("target")).expect("mkdir");
        fs::write(root.join("target/out.o"), "ignored").expect("write");
        let after = cp.snapshot(&roots).await.expect("snapshot");

        let changes = cp.changes(&before, &after).await.expect("changes");
        assert_eq!(
            changes,
            vec![
                Change {
                    path: root.join("gone.txt"),
                    deleted: true,
                    before: Before::Bytes(b"bye\n".to_vec()),
                },
                Change {
                    path: root.join("made.txt"),
                    deleted: false,
                    before: Before::Absent,
                },
                Change {
                    path: root.join("src/a.rs"),
                    deleted: false,
                    before: Before::Bytes(b"one\n".to_vec()),
                },
            ]
        );
        // The private repository never touched the workspace.
        assert!(!root.join(".git").exists());
    }

    #[tokio::test]
    async fn snapshot_reuses_the_real_repository_objects() {
        if !have_git().await {
            return;
        }
        let (_dir, root, cp) = workspace();
        let git = |args: &[&str]| {
            let root = root.clone();
            let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            async move {
                Command::new("git")
                    .current_dir(&root)
                    .args(&args)
                    .output()
                    .await
                    .expect("git")
                    .status
                    .success()
            }
        };
        assert!(git(&["init", "-q"]).await);
        assert!(git(&["add", "-A"]).await);
        let roots = vec![root.clone()];
        let snap = cp.snapshot(&roots).await.expect("snapshot");
        assert_eq!(snap.trees.len(), 1);
        // `src/a.rs` is already a blob in the real repository, so the
        // private object store holds only the trees it wrote.
        let private = cp.git_dir(&root).join("objects");
        let blobs = walk(&private)
            .into_iter()
            .filter(|p| {
                p.parent()
                    .and_then(|d| d.file_name())
                    .is_some_and(|n| n.len() == 2)
            })
            .count();
        assert!(blobs <= 3, "expected only tree objects, found {blobs}");
    }

    #[tokio::test]
    async fn preimages_confine_and_read_named_paths() {
        let (_dir, root, cp) = workspace();
        let got = cp
            .preimages(
                std::slice::from_ref(&root),
                &root,
                &["src/a.rs".into(), "new.rs".into(), "../escape".into()],
            )
            .await;
        assert_eq!(
            got,
            vec![
                PreImage {
                    path: root.join("src/a.rs"),
                    before: Before::Bytes(b"one\n".to_vec()),
                },
                PreImage {
                    path: root.join("new.rs"),
                    before: Before::Absent,
                },
            ]
        );
    }

    #[tokio::test]
    async fn restore_writes_bytes_back_and_removes_created_files() {
        let (_dir, root, cp) = workspace();
        let roots = vec![root.clone()];
        cp.restore(&roots, &root, &root.join("src/a.rs"), Some(b"zero\n"))
            .await
            .expect("write back");
        assert_eq!(fs::read(root.join("src/a.rs")).expect("read"), b"zero\n");
        cp.restore(&roots, &root, &root.join(".gitignore"), None)
            .await
            .expect("remove");
        assert!(!root.join(".gitignore").exists());
        // Removing twice is fine; escaping the root is not.
        cp.restore(&roots, &root, &root.join(".gitignore"), None)
            .await
            .expect("idempotent");
        assert!(
            cp.restore(&roots, &root, &root.join("../escape"), Some(b"x"))
                .await
                .is_err()
        );
    }

    /// Local only: a warm snapshot over a large tree is one stat pass.
    /// Run with `--ignored`; the number goes into `done.md`.
    #[tokio::test]
    #[ignore]
    async fn warm_snapshot_under_200ms_on_50k_files() {
        let (_dir, root, cp) = workspace();
        for d in 0..500 {
            let dir = root.join(format!("d{d}"));
            fs::create_dir_all(&dir).expect("mkdir");
            for f in 0..100 {
                fs::write(dir.join(format!("f{f}.txt")), format!("{d}-{f}\n")).expect("write");
            }
        }
        let roots = vec![root.clone()];
        cp.snapshot(&roots).await.expect("cold snapshot");
        let started = std::time::Instant::now();
        cp.snapshot(&roots).await.expect("warm snapshot");
        let elapsed = started.elapsed();
        assert!(elapsed.as_millis() < 200, "warm snapshot took {elapsed:?}");
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(entries) = fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    out.extend(walk(&p));
                } else {
                    out.push(p);
                }
            }
        }
        out
    }
}
