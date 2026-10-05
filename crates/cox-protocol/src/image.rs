// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The one image check (P40, T40.1): sniffs PNG, JPEG, GIF and WebP by magic
//! bytes, caps the size, and encodes the result as an [`Attachment`] or a
//! tool-output payload. Surfaces, `read` and the core all call it, so there
//! is no second check anywhere. It lives in the contract crate because every
//! one of those callers already depends on it, and it is pure: bytes in,
//! bytes out, no filesystem (D2).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use thiserror::Error;

use crate::types::{Attachment, ToolOutput};

/// The largest image cox sends, in raw bytes. Why: 5,000,000 base64 bytes,
/// the smallest documented per-image limit (Bedrock and Google Cloud for
/// Anthropic models; plan.md P40 intro).
pub const MAX_IMAGE_BYTES: usize = 3_750_000;

/// Tokens counted for one image before the provider reports usage. Why: the
/// standard-tier cap of 1568 visual tokens, rounded; the reported usage
/// corrects it.
pub const IMAGE_TOKEN_ESTIMATE: u64 = 1600;

/// The key under `ToolOutput::structured` that carries a tool's image.
/// Why a key and not a field: `ToolOutput` has dozens of literal
/// constructions, and a field would touch every one.
const STRUCTURED_KEY: &str = "image";

/// Why bytes or an attachment are not an image cox sends.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ImageError {
    /// The bytes are not PNG, JPEG, GIF or WebP.
    #[error("not a PNG, JPEG, GIF or WebP image")]
    NotAnImage,
    /// The decoded image is over [`MAX_IMAGE_BYTES`].
    #[error("image is {bytes} bytes, over the {cap}-byte cap")]
    TooLarge {
        /// The image's size in raw bytes.
        bytes: usize,
        /// The cap it exceeds.
        cap: usize,
    },
    /// The declared media type is not what the bytes are.
    #[error("declared {declared}, but the bytes are {sniffed}")]
    MediaTypeMismatch {
        /// The media type the attachment claims.
        declared: String,
        /// The media type the bytes carry.
        sniffed: &'static str,
    },
    /// `data_b64` is not standard padded base64.
    #[error("image data is not valid base64")]
    BadBase64,
}

/// The media type of `bytes` by magic number, or `None` for anything but the
/// four formats Anthropic and OpenAI both accept.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG") {
        Some("image/png")
    } else if bytes.starts_with(b"\xFF\xD8\xFF") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()) {
        Some("image/webp")
    } else {
        None
    }
}

/// Checks raw bytes and wraps them as an attachment named `name`.
pub fn attachment(name: impl Into<String>, bytes: &[u8]) -> Result<Attachment, ImageError> {
    let media_type = check(bytes)?;
    Ok(Attachment {
        name: name.into(),
        media_type: media_type.to_owned(),
        data_b64: STANDARD.encode(bytes),
    })
}

/// Checks an attachment that arrived already encoded (a surface, a resumed
/// rollout): size, base64, format, and that the declared type matches.
pub fn validate(attachment: &Attachment) -> Result<(), ImageError> {
    let b64 = attachment.data_b64.as_bytes();
    // The decoded length follows from the encoded one, so an oversized image
    // is refused before any allocation, and the decode below stays bounded.
    let padding = b64.iter().rev().take_while(|&&c| c == b'=').count().min(2);
    let bytes = (b64.len() / 4 * 3).saturating_sub(padding);
    cap(bytes)?;
    let decoded = STANDARD.decode(b64).map_err(|_| ImageError::BadBase64)?;
    let sniffed = check(&decoded)?;
    if !attachment.media_type.eq_ignore_ascii_case(sniffed) {
        return Err(ImageError::MediaTypeMismatch {
            declared: attachment.media_type.clone(),
            sniffed,
        });
    }
    Ok(())
}

/// The `ToolOutput::structured` payload for an image a tool returns.
/// `media_type` is what [`sniff`] said about `bytes`.
pub fn to_structured(media_type: &str, bytes: &[u8]) -> Value {
    json!({
        STRUCTURED_KEY: { "media_type": media_type, "data_b64": STANDARD.encode(bytes) }
    })
}

/// Removes the image payload from `out` and returns `(media_type,
/// data_b64)`. A payload left empty is dropped, so the text-only view of the
/// output is what it would be without the image.
pub fn take_structured(out: &mut ToolOutput) -> Option<(String, String)> {
    let map = out.structured.as_mut()?.as_object_mut()?;
    let image = map.get(STRUCTURED_KEY)?;
    let media_type = image.get("media_type")?.as_str()?.to_owned();
    let data_b64 = image.get("data_b64")?.as_str()?.to_owned();
    map.shift_remove(STRUCTURED_KEY);
    if map.is_empty() {
        out.structured = None;
    }
    Some((media_type, data_b64))
}

fn cap(bytes: usize) -> Result<(), ImageError> {
    if bytes > MAX_IMAGE_BYTES {
        return Err(ImageError::TooLarge {
            bytes,
            cap: MAX_IMAGE_BYTES,
        });
    }
    Ok(())
}

fn check(bytes: &[u8]) -> Result<&'static str, ImageError> {
    cap(bytes.len())?;
    sniff(bytes).ok_or(ImageError::NotAnImage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    const JPEG: &[u8] = b"\xff\xd8\xff\xe0\0\x10JFIF\0";
    const GIF87: &[u8] = b"GIF87a\x01\0\x01\0";
    const GIF89: &[u8] = b"GIF89a\x01\0\x01\0";
    const WEBP: &[u8] = b"RIFF\x24\0\0\0WEBPVP8 ";

    fn encoded(media_type: &str, bytes: &[u8]) -> Attachment {
        Attachment {
            name: "x".into(),
            media_type: media_type.into(),
            data_b64: STANDARD.encode(bytes),
        }
    }

    fn output(structured: Value) -> ToolOutput {
        ToolOutput {
            text: String::new(),
            is_error: false,
            diff: None,
            structured: Some(structured),
        }
    }

    #[rstest]
    #[case::png(PNG, "image/png")]
    #[case::jpeg(JPEG, "image/jpeg")]
    #[case::gif87a(GIF87, "image/gif")]
    #[case::gif89a(GIF89, "image/gif")]
    #[case::webp(WEBP, "image/webp")]
    fn sniff_names_each_accepted_format(#[case] bytes: &[u8], #[case] want: &str) {
        assert_eq!(sniff(bytes), Some(want));
    }

    #[rstest]
    #[case::text(b"hello, world".as_slice())]
    #[case::empty(b"".as_slice())]
    #[case::riff_wave(b"RIFF\x24\0\0\0WAVEfmt ".as_slice())]
    #[case::truncated_png(b"\x89PN".as_slice())]
    fn sniff_refuses_anything_else(#[case] bytes: &[u8]) {
        assert_eq!(sniff(bytes), None);
    }

    #[test]
    fn attachment_at_the_cap_is_accepted_and_one_byte_over_is_too_large() {
        let mut bytes = PNG.to_vec();
        bytes.resize(MAX_IMAGE_BYTES, 0);
        let ok = attachment("big.png", &bytes).map(|a| a.media_type);
        assert_eq!(ok, Ok("image/png".to_owned()));

        bytes.push(0);
        assert_eq!(
            attachment("big.png", &bytes),
            Err(ImageError::TooLarge {
                bytes: MAX_IMAGE_BYTES + 1,
                cap: MAX_IMAGE_BYTES
            })
        );
    }

    #[test]
    fn attachment_refuses_bytes_that_are_not_an_image() {
        assert_eq!(
            attachment("a.txt", b"plain text"),
            Err(ImageError::NotAnImage)
        );
    }

    #[test]
    fn attachment_round_trips_through_validate() {
        let a = attachment("shot.png", PNG).map_err(|e| e.to_string());
        let a = a.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(a.name, "shot.png");
        assert_eq!(a.data_b64, STANDARD.encode(PNG));
        assert_eq!(validate(&a), Ok(()));
    }

    #[test]
    fn validate_refuses_over_cap_base64() {
        let mut bytes = PNG.to_vec();
        bytes.resize(MAX_IMAGE_BYTES + 3, 0);
        assert_eq!(
            validate(&encoded("image/png", &bytes)),
            Err(ImageError::TooLarge {
                bytes: MAX_IMAGE_BYTES + 3,
                cap: MAX_IMAGE_BYTES
            })
        );
    }

    #[test]
    fn validate_refuses_a_declared_type_the_bytes_contradict() {
        assert_eq!(
            validate(&encoded("image/png", JPEG)),
            Err(ImageError::MediaTypeMismatch {
                declared: "image/png".into(),
                sniffed: "image/jpeg"
            })
        );
    }

    #[test]
    fn validate_refuses_bad_base64_even_after_a_valid_prefix() {
        let mut a = encoded("image/png", PNG);
        a.data_b64.push_str("!!!!");
        assert_eq!(validate(&a), Err(ImageError::BadBase64));
    }

    #[test]
    fn validate_refuses_encoded_bytes_that_are_not_an_image() {
        assert_eq!(
            validate(&encoded("image/png", b"not an image")),
            Err(ImageError::NotAnImage)
        );
    }

    #[test]
    fn take_structured_returns_the_image_and_drops_the_emptied_payload() {
        let mut out = output(to_structured("image/gif", GIF89));
        assert_eq!(
            take_structured(&mut out),
            Some(("image/gif".to_owned(), STANDARD.encode(GIF89)))
        );
        assert_eq!(out.structured, None);
    }

    #[test]
    fn take_structured_keeps_other_keys_and_ignores_outputs_without_an_image() {
        let mut payload = to_structured("image/webp", WEBP);
        if let Some(map) = payload.as_object_mut() {
            map.insert("lines".into(), json!(3));
        }
        let mut out = output(payload);
        assert!(take_structured(&mut out).is_some());
        assert_eq!(out.structured, Some(json!({ "lines": 3 })));
        assert_eq!(take_structured(&mut out), None);
        assert_eq!(out.structured, Some(json!({ "lines": 3 })));
    }
}
