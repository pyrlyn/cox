// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The composer's attachment row (DS§6.4 row `Composer`, the mockup's `.composer` `atts` row;
// DT§5.3): each file or image the next message carries as a `Thumbnail`, with a way to take it
// back out. Separate from `Composer.swift` so the organism's file stays one view, and the tile
// with its remove badge is drawn in one place.

import AppKit
import SwiftUI

/// A row of `Thumbnail`s, an `xmark.circle.fill` badge on each tile's corner.
struct ComposerAttachments: View {
  let attachments: [Composer.Attachment]
  let remove: (String) -> Void

  var body: some View {
    HStack(spacing: Space.m) {
      ForEach(attachments) { attachment in
        Thumbnail(attachment.name, image: attachment.image.flatMap(Self.picture))
          .overlay(alignment: .topTrailing) {
            Button {
              remove(attachment.id)
            } label: {
              Image(systemName: "xmark.circle.fill")
                .symbolStyle(.body)
                // The cross on a disc of the window's face, so it reads over any picture.
                .symbolRenderingMode(.palette)
                .foregroundStyle(Color(.textSecondary), Color(.surfaceWindow))
            }
            .buttonStyle(.plain)
            .padding(Space.xxs)
            .help("Remove \(attachment.name)")
            .accessibilityLabel("Remove \(attachment.name)")
          }
      }
    }
  }

  /// An image's bytes as a picture; bytes AppKit cannot read draw the file tile instead.
  private static func picture(_ data: Data) -> Image? {
    NSImage(data: data).map { Image(nsImage: $0) }
  }
}
