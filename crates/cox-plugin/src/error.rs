// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `PluginError`: everything a plugin load or call can fail with. Its own
//! module because `extism::Error` is an untyped anyhow wrapper (R§4.3.5 P6),
//! so the one mapping into a typed error lives here and nowhere else.

use thiserror::Error;

/// A plugin load or call failed. Callers fail open on every variant
/// (plan.md §1.15 invariant 17): the plugin is skipped, never fatal.
#[derive(Debug, Error)]
pub enum PluginError {
    /// The module did not compile or instantiate, or its worker could not start.
    #[error("plugin failed to load: {0}")]
    Load(String),
    /// A required export is missing (only `cox_init` is required, PL§4).
    #[error("plugin does not export `{0}`")]
    MissingExport(&'static str),
    /// The call ran past its deadline and was cancelled.
    #[error("plugin call `{export}` passed its deadline and was cancelled")]
    Timeout {
        /// The export that was cancelled.
        export: String,
    },
    /// The guest tried to grow memory past the plugin's cap.
    #[error("plugin call `{export}` hit the memory cap")]
    OutOfMemory {
        /// The export that trapped.
        export: String,
    },
    /// The guest trapped or a host function refused it.
    #[error("plugin call `{export}` failed: {message}")]
    Trap {
        /// The export that failed.
        export: String,
        /// extism's root cause, as text.
        message: String,
    },
    /// A payload did not (de)serialise as the ABI type.
    #[error("plugin payload is not the expected JSON: {0}")]
    Payload(#[from] serde_json::Error),
    /// The queue for this lane is full.
    #[error("plugin queue is full")]
    Busy,
    /// The worker has shut down.
    #[error("plugin worker has stopped")]
    Stopped,
}

impl PluginError {
    /// Maps a failed `Plugin::call`. extism reports its own two limits as
    /// bare messages: `"timeout"` for an epoch interrupt (a `CancelHandle`
    /// or the manifest `timeout_ms`) and `"oom"` from its memory limiter
    /// (`extism-1.30.0/src/plugin.rs:1079-1092`).
    pub(crate) fn from_call(export: &str, err: &extism::Error) -> Self {
        let export = export.to_string();
        match err.root_cause().to_string().as_str() {
            "timeout" => Self::Timeout { export },
            "oom" => Self::OutOfMemory { export },
            message => Self::Trap {
                export,
                message: message.to_string(),
            },
        }
    }
}
