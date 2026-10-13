// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The bytes of a widget image (PL§15.4, T33.45.6): a PNG the plugin ships in
//! its package. Separate from `external_agent` because that module resolves
//! programs to run, while this one only reads, and a host that draws a
//! plugin's image must read exactly what the package digest covers: a
//! regular file under the package, through no symlink, small, and a PNG.
//! Never a URL, so the host fetches nothing for a plugin.

use std::io::Read;
use std::path::Path;

use cox_plugin_api::{MAX_IMAGE_BYTES, is_package_path};
use cox_protocol::image::sniff;
use thiserror::Error;

use crate::external_agent::{ProgramError, package_file};

/// Why an image is not drawn; the widget shows its `alt` text instead.
#[derive(Debug, Error)]
pub enum ImageError {
    /// Absolute, empty, or with a `..`, `\` or `:` component.
    #[error("image path {0:?} is not a path inside the package")]
    Path(String),
    /// Not a regular file inside the package, or a symlink.
    #[error(transparent)]
    File(#[from] ProgramError),
    /// Over the size cap.
    #[error("image is larger than {MAX_IMAGE_BYTES} bytes")]
    TooLarge,
    /// The bytes do not start with the PNG signature.
    #[error("image is not a PNG")]
    NotPng,
    /// The file could not be read.
    #[error("image: {0}")]
    Io(#[from] std::io::Error),
}

/// The PNG at `rel` inside the package at `dir`.
pub fn package_image(dir: &Path, rel: &str) -> Result<Vec<u8>, ImageError> {
    if !is_package_path(rel) {
        return Err(ImageError::Path(rel.to_string()));
    }
    let file = package_file(dir, rel)?;
    // One byte more than the cap, so a file that grew since the check shows.
    let limit = MAX_IMAGE_BYTES as u64 + 1;
    let mut bytes = Vec::new();
    std::fs::File::open(file)?
        .take(limit)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(ImageError::TooLarge);
    }
    // The magic number, as an attachment's image is typed (`cox_protocol::image`).
    if sniff(&bytes) != Some("image/png") {
        return Err(ImageError::NotPng);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> Vec<u8> {
        b"\x89PNG\r\n\x1a\nrest".to_vec()
    }

    #[test]
    fn package_image_reads_only_a_small_png_inside_the_package() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir(root.join("img")).expect("dir");
        std::fs::write(root.join("img/a.png"), png()).expect("png");
        std::fs::write(root.join("text.png"), b"not a png").expect("text");
        let mut big = png();
        big.resize(MAX_IMAGE_BYTES + 1, 0);
        std::fs::write(root.join("big.png"), &big).expect("big");
        big.truncate(MAX_IMAGE_BYTES);
        std::fs::write(root.join("max.png"), &big).expect("max");

        assert_eq!(package_image(root, "img/a.png").expect("reads"), png());
        assert!(package_image(root, "max.png").is_ok());
        assert!(matches!(
            package_image(root, "big.png"),
            Err(ImageError::TooLarge)
        ));
        assert!(matches!(
            package_image(root, "text.png"),
            Err(ImageError::NotPng)
        ));
        for outside in ["../a.png", "/etc/passwd", "img\\a.png", "C:a.png", ""] {
            assert!(
                matches!(package_image(root, outside), Err(ImageError::Path(_))),
                "{outside:?}"
            );
        }
        assert!(matches!(
            package_image(root, "img"),
            Err(ImageError::File(ProgramError::NotFile(_)))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn package_image_refuses_a_symlink() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let outside = tempfile::tempdir().expect("tempdir");
        std::fs::write(outside.path().join("secret.png"), png()).expect("png");
        std::os::unix::fs::symlink(outside.path().join("secret.png"), root.join("link.png"))
            .expect("symlink");
        assert!(matches!(
            package_image(root, "link.png"),
            Err(ImageError::File(ProgramError::Symlink(_)))
        ));
    }
}
