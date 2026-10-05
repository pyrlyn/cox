// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! Push-to-talk dictation (P54, A123): local speech-to-text with whisper.cpp.
//! Its own crate under D1 because whisper.cpp is a heavy C++ build (like the
//! grammars in `cox-syntax`); `crates/cox` links it only behind its `voice`
//! feature, off by default. Audio never leaves the process: nothing here
//! opens a socket or writes a file, and captured samples live only in memory
//! until they are transcribed or dropped. `PushToTalk` joins the two
//! behind `cox_protocol`'s `Dictation`, the one thing the TUI sees.

// why: this crate was not under `missing_docs` before the workspace lints;
// drop this allow once its public items are documented.
#![allow(missing_docs)]

mod capture;
mod transcribe;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cox_protocol::traits::{Dictation, DictationError};
use tokio::task::JoinHandle;

pub use capture::{Recorder, WHISPER_RATE, input_device};
pub use transcribe::Transcriber;

/// The pinned model table `cox-vendor whisper-models` writes (T54.1): per
/// model its name, file, download URL at a fixed commit, SHA-256 and size.
/// `cox voice model` reads it; this crate never downloads anything.
pub const MODELS_JSON: &str = include_str!("../data/whisper-models.json");

/// What can go wrong between a model file and a transcript.
#[derive(Debug, thiserror::Error)]
pub enum VoiceError {
    #[error("whisper model not found at {0}; run `cox voice model download <name>`")]
    ModelMissing(PathBuf),
    #[error("{path} is not a whisper ggml model: {reason}")]
    ModelInvalid { path: PathBuf, reason: String },
    #[error("whisper: {0}")]
    Whisper(String),
    #[error(
        "no microphone found; check that one is connected and that this terminal may use it (macOS: System Settings > Privacy & Security > Microphone)"
    )]
    NoInputDevice,
    #[error(
        "microphone: {0}; if the OS denied access, allow this terminal to use the microphone (macOS: System Settings > Privacy & Security > Microphone)"
    )]
    Stream(String),
}

impl From<VoiceError> for DictationError {
    fn from(e: VoiceError) -> Self {
        match e {
            VoiceError::NoInputDevice | VoiceError::Stream(_) => Self::Capture(e.to_string()),
            _ => Self::Transcribe(e.to_string()),
        }
    }
}

/// The model's state across presses: loading starts at the first press so
/// it overlaps the speech, and a failed load is retried at the next one.
enum Model {
    Unloaded,
    Loading(JoinHandle<Result<Transcriber, VoiceError>>),
    Ready(Arc<Transcriber>),
}

/// Push-to-talk over the default microphone and one whisper model: the
/// `Dictation` `crates/cox` hands the TUI. The C++ work (loading,
/// transcribing) runs on blocking threads, never on the TUI's runtime.
pub struct PushToTalk {
    model_path: PathBuf,
    language: Option<String>,
    max: Duration,
    recorder: Option<Recorder>,
    model: Model,
}

impl PushToTalk {
    /// `language` is an ISO-639-1 code, `None` to let whisper detect it;
    /// `max` caps one recording.
    pub fn new(model_path: PathBuf, language: Option<String>, max: Duration) -> Self {
        Self {
            model_path,
            language,
            max,
            recorder: None,
            model: Model::Unloaded,
        }
    }

    async fn transcriber(&mut self) -> Result<Arc<Transcriber>, VoiceError> {
        let loaded = match std::mem::replace(&mut self.model, Model::Unloaded) {
            Model::Ready(t) => Ok(t),
            Model::Loading(handle) => joined(handle.await),
            Model::Unloaded => joined(load(self.model_path.clone()).await),
        }?;
        self.model = Model::Ready(Arc::clone(&loaded));
        Ok(loaded)
    }
}

fn load(path: PathBuf) -> JoinHandle<Result<Transcriber, VoiceError>> {
    tokio::task::spawn_blocking(move || Transcriber::load(&path))
}

fn joined(
    r: Result<Result<Transcriber, VoiceError>, tokio::task::JoinError>,
) -> Result<Arc<Transcriber>, VoiceError> {
    r.map_err(|e| VoiceError::Whisper(e.to_string()))?
        .map(Arc::new)
}

#[async_trait::async_trait]
impl Dictation for PushToTalk {
    fn start(&mut self) -> Result<(), DictationError> {
        self.recorder = Some(Recorder::start(self.max)?);
        if matches!(self.model, Model::Unloaded) && tokio::runtime::Handle::try_current().is_ok() {
            self.model = Model::Loading(load(self.model_path.clone()));
        }
        Ok(())
    }

    async fn stop(&mut self) -> Result<String, DictationError> {
        let recorder = self
            .recorder
            .take()
            .ok_or_else(|| DictationError::Capture("not recording".into()))?;
        let transcriber = self.transcriber().await?;
        let language = self.language.clone();
        tokio::task::spawn_blocking(move || {
            let pcm = recorder.stop()?;
            transcriber.transcribe(&pcm, language.as_deref())
        })
        .await
        .map_err(|e| DictationError::Transcribe(e.to_string()))?
        .map_err(DictationError::from)
    }

    fn cancel(&mut self) {
        self.recorder = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stop_without_a_press_is_a_capture_error() {
        let mut ptt = PushToTalk::new("missing.bin".into(), None, Duration::from_secs(1));
        let err = ptt.stop().await.expect_err("nothing recorded");
        assert!(matches!(err, DictationError::Capture(_)), "{err}");
    }

    #[tokio::test]
    async fn a_missing_model_is_a_transcribe_error_and_is_retried() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut ptt = PushToTalk::new(
            dir.path().join("ggml-base.en.bin"),
            None,
            Duration::from_secs(1),
        );
        let err = DictationError::from(ptt.transcriber().await.err().expect("missing"));
        assert!(
            matches!(&err, DictationError::Transcribe(t) if t.contains("not found")),
            "{err}"
        );
        assert!(matches!(ptt.model, Model::Unloaded));
    }

    #[test]
    fn device_errors_are_capture_errors() {
        assert!(matches!(
            DictationError::from(VoiceError::NoInputDevice),
            DictationError::Capture(_)
        ));
        assert!(matches!(
            DictationError::from(VoiceError::Whisper("x".into())),
            DictationError::Transcribe(_)
        ));
    }
}
