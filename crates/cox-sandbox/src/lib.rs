// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The two filesystem/process trust guards (T32.3; `docs/design/crates.md`
//! C3): `path::confine`, the one place a filesystem path from the model is
//! checked against the workspace roots, and `sandbox`, the front door that
//! turns a shell command plus the session's `SandboxPolicy` into the
//! `Command` that confines it on this host (Seatbelt on macOS, bubblewrap
//! or Landlock+seccomp on Linux).
//!
//! Separate from `cox-tools` (dependencies (a): `landlock` and
//! `seccompiler` are Linux-only and platform-gated, and guard (b): both
//! modules are trust boundaries named in AGENTS.md and SECURITY.md) so a
//! consumer that only needs the guards does not pull in the rest of the
//! tool set. `cox-tools` re-exports both modules at their old paths, so
//! `cox_tools::path::confine` and `cox_tools::sandbox::Policy` keep working
//! for existing callers.

// why: this crate was not under `missing_docs` before the workspace lints;
// drop this allow once its public items are documented.
#![allow(missing_docs)]

pub mod path;
pub mod sandbox;
