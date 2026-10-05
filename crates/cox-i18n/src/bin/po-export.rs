// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `po-export`: writes the native apps' string resources from the embedded
//! gettext catalogs (`cox_i18n::export`). Output defaults to `target/i18n/`
//! under the workspace root, which is gitignored; the macOS and Windows
//! builds run this and copy from there (`docs/i18n.md`).
//!
//!     cargo run -p cox-i18n --bin po-export [-- --out <dir>]

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let out = match (args.next().as_deref(), args.next()) {
        (None, _) => {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
            root.canonicalize().unwrap_or(root).join("target/i18n")
        }
        (Some("--out"), Some(dir)) => PathBuf::from(dir),
        _ => {
            eprintln!("usage: po-export [--out <dir>]");
            return ExitCode::from(2);
        }
    };
    match cox_i18n::export::export_all(&out) {
        Ok(files) => {
            for file in files {
                println!("{}", file.display());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("po-export: {e}");
            ExitCode::FAILURE
        }
    }
}
