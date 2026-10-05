// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Re-exports `cox-telemetry` (T32.9) at its old path so `main.rs` keeps
//! working unchanged. The crate itself lives at `crates/cox-telemetry`
//! because it is the only user of five otel crates (dependency (a)).

pub use cox_telemetry::*;
