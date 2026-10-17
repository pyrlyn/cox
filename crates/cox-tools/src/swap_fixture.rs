// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Test fixture for the symlink-swap window (T64.15): a workspace root, a
//! sibling directory outside it holding `secret.txt`, and the two swaps an
//! attacker makes after `confine` approved a path. Shared by the `read`,
//! `edit`, `write` and `apply_patch` tests so each call site proves the same
//! refusal without four copies of the setup.

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

pub(crate) const SECRET: &str = "secret";

pub(crate) struct Swap {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
    pub outside: PathBuf,
}

impl Swap {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = std::fs::canonicalize(dir.path()).expect("canonicalize");
        let root = base.join("root");
        let outside = base.join("outside");
        std::fs::create_dir(&root).expect("mkdir root");
        std::fs::create_dir(&outside).expect("mkdir outside");
        std::fs::write(outside.join("secret.txt"), SECRET).expect("write secret");
        Self {
            _dir: dir,
            root,
            outside,
        }
    }

    /// Writes `rel` (under the root) with `content`, creating its parents.
    pub fn file(&self, rel: &str, content: &str) -> PathBuf {
        let path = self.root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).expect("mkdir");
        std::fs::write(&path, content).expect("write");
        path
    }

    /// Replaces the file `rel` with a symlink to the outside secret.
    pub fn swap_leaf(&self, rel: &str) {
        let path = self.root.join(rel);
        std::fs::remove_file(&path).expect("rm leaf");
        symlink(self.outside.join("secret.txt"), &path).expect("symlink leaf");
    }

    /// Replaces the directory `rel` with a symlink to the outside directory.
    pub fn swap_parent(&self, rel: &str) {
        let dir = self.root.join(rel);
        std::fs::rename(&dir, self.root.join(format!("{rel}.moved"))).expect("rename");
        symlink(&self.outside, &dir).expect("symlink parent");
    }

    /// Plants a symlink to the outside directory where the not-yet-created
    /// directory `rel` would go, so a later `create_dir_all` follows it.
    pub fn plant_parent(&self, rel: &str) {
        symlink(&self.outside, self.root.join(rel)).expect("symlink planted parent");
    }

    pub fn outside_secret(&self) -> String {
        std::fs::read_to_string(self.outside.join("secret.txt")).expect("read secret")
    }
}
