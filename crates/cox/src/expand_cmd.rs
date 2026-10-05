// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `cox expand`: reads an archived tool result from the configured Store,
//! using the same `lines=` grammar as the `expand` tool.

use std::path::Path;

use cox_protocol::{ArchiveId, Store as _};
use cox_store::Store;
use cox_tools::expand::{parse_range, select_lines};

pub fn run(home: &Path, raw_id: &str, lines: Option<&str>) -> anyhow::Result<()> {
    let id: ArchiveId = raw_id.parse()?;
    let bytes = Store::open(home)?.archive_get(&id)?;
    let text = String::from_utf8_lossy(&bytes);
    let range = match lines {
        Some(raw) => Some(parse_range(raw).ok_or_else(|| anyhow::anyhow!("bad --lines {raw:?}"))?),
        None => None,
    };
    let out = select_lines(&text, range);
    if range.is_some() {
        println!("{out}");
    } else {
        print!("{out}");
    }
    Ok(())
}
