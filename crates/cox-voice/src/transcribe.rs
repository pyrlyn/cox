// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! `Transcriber`: one loaded whisper.cpp model turning a 16 kHz mono buffer
//! into text. Separate from capture so it tests without a microphone, and so
//! the model (tens to hundreds of MB) is loaded once and reused per press.

use std::io::Read;
use std::path::Path;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::VoiceError;

/// The first four bytes of every ggml model whisper.cpp reads: the magic
/// `0x67676d6c` ("ggml") as a little-endian `u32`. Checked here so a wrong
/// file is a typed error before any C++ touches it.
const GGML_MAGIC: [u8; 4] = 0x6767_6d6c_u32.to_le_bytes();

/// A loaded model. Transcription takes `&self`, so one instance serves
/// every press of the key.
pub struct Transcriber {
    ctx: WhisperContext,
}

impl Transcriber {
    /// Loads the ggml model at `model`, once.
    pub fn load(model: &Path) -> Result<Self, VoiceError> {
        if !model.is_file() {
            return Err(VoiceError::ModelMissing(model.to_path_buf()));
        }
        let invalid = |reason: String| VoiceError::ModelInvalid {
            path: model.to_path_buf(),
            reason,
        };
        let mut magic = [0u8; 4];
        std::fs::File::open(model)
            .and_then(|mut f| f.read_exact(&mut magic))
            .map_err(|e| invalid(e.to_string()))?;
        if magic != GGML_MAGIC {
            return Err(invalid("no ggml magic at the start of the file".into()));
        }
        // Idempotent: whisper.cpp and ggml log through `tracing` from here
        // on, never to stderr under the TUI.
        whisper_rs::install_logging_hooks();
        let ctx = WhisperContext::new_with_params(model, WhisperContextParameters::default())
            .map_err(|e| invalid(e.to_string()))?;
        Ok(Self { ctx })
    }

    /// Greedy decoding without timestamps over 16 kHz mono `f32` samples;
    /// returns the trimmed text of all segments joined by single spaces.
    /// `language` is an ISO-639-1 code; `None` lets whisper detect it.
    pub fn transcribe(
        &self,
        pcm_16k_mono: &[f32],
        language: Option<&str>,
    ) -> Result<String, VoiceError> {
        if pcm_16k_mono.is_empty() {
            return Ok(String::new());
        }
        let whisper = |e: whisper_rs::WhisperError| VoiceError::Whisper(e.to_string());
        let mut state = self.ctx.create_state().map_err(whisper)?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(threads());
        params.set_language(language);
        params.set_no_timestamps(true);
        params.set_no_context(true);
        params.set_suppress_blank(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        state.full(params, pcm_16k_mono).map_err(whisper)?;
        let mut words = Vec::new();
        for i in 0..state.full_n_segments() {
            let Some(segment) = state.get_segment(i) else {
                continue;
            };
            let text = segment.to_str_lossy().map_err(whisper)?;
            if !is_annotation(text.trim()) {
                words.extend(text.split_whitespace().map(str::to_owned));
            }
        }
        Ok(words.join(" "))
    }
}

/// whisper's non-speech markers (`[BLANK_AUDIO]`, `(wind blowing)`): a
/// whole segment in brackets is a description of the audio, not dictation.
fn is_annotation(segment: &str) -> bool {
    (segment.starts_with('[') && segment.ends_with(']'))
        || (segment.starts_with('(') && segment.ends_with(')'))
}

/// Decoding threads: the machine's parallelism, capped so a dictation never
/// starves the rest of the session.
fn threads() -> i32 {
    std::thread::available_parallelism().map_or(4, |n| n.get().clamp(1, 8) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcriber_rejects_a_missing_model() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ggml-base.en.bin");
        let err = Transcriber::load(&path)
            .err()
            .expect("no model, no transcriber");
        assert!(matches!(err, VoiceError::ModelMissing(p) if p == path));
    }

    #[test]
    fn transcriber_rejects_a_file_that_is_not_ggml() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ggml-base.en.bin");
        std::fs::write(&path, b"<html>404 Not Found</html>").expect("write");
        let err = Transcriber::load(&path).err().expect("html is not a model");
        assert!(matches!(err, VoiceError::ModelInvalid { .. }), "{err:?}");
        std::fs::write(&path, b"lm").expect("write");
        let err = Transcriber::load(&path)
            .err()
            .expect("two bytes are not a model");
        assert!(matches!(err, VoiceError::ModelInvalid { .. }), "{err:?}");
    }

    #[test]
    fn bracketed_segments_are_annotations_not_speech() {
        assert!(is_annotation("[BLANK_AUDIO]"));
        assert!(is_annotation("(wind blowing)"));
        assert!(!is_annotation("call foo(bar)"));
        assert!(!is_annotation(""));
    }

    /// Needs a real model: `COX_WHISPER_MODEL=/path/to/ggml-tiny.en.bin
    /// cargo nextest run -p cox-voice --run-ignored only
    /// transcribe_of_silence_is_empty`. Never in CI (no model download there).
    #[test]
    #[ignore = "needs a downloaded ggml model in COX_WHISPER_MODEL"]
    fn transcribe_of_silence_is_empty() {
        let model = std::env::var_os("COX_WHISPER_MODEL").expect("COX_WHISPER_MODEL is set");
        let t = Transcriber::load(Path::new(&model)).expect("model loads");
        let silence = vec![0.0f32; 16_000 * 2];
        let text = t.transcribe(&silence, Some("en")).expect("transcribes");
        assert_eq!(text, "");
    }
}
