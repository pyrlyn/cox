// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `Thumbnail` (DS§6.2 row `Thumbnail(attachment)`, the mockup's `.thumb`): one attachment of a
// user turn, as a small tile. Separate so the composer and the user bubble show an attachment
// the same way. Takes the attachment's name and, for an image, its picture as plain values:
// CoxUI depends on no other cox package, so it never sees `CoxClient.Attachment`.

import SwiftUI

/// An image attachment fills the tile with its picture; any other file shows the document
/// symbol over its name. Both sit in a rounded tile with a hairline rim.
public struct Thumbnail: View {
  let name: String
  /// The picture of an image attachment; `nil` draws the file variant.
  let image: Image?

  /// The mockup's `.thumb` box, 92 × 60: no size token fits a tile this shape.
  private static let width: CGFloat = 92
  private static let height: CGFloat = 60

  public init(_ name: String, image: Image? = nil) {
    self.name = name
    self.image = image
  }

  public var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.m, style: .continuous)
    Group {
      if let image {
        image.resizable().scaledToFill()
      } else {
        FileFace(name: name)
      }
    }
    .frame(width: Self.width, height: Self.height)
    .clipShape(shape)
    .hairline(in: shape)
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(name)
    .accessibilityAddTraits(image == nil ? [] : .isImage)
  }
}

/// The file variant: the DS§3.7 `doc` symbol above the name, which keeps its extension visible
/// when it is too long for the tile.
private struct FileFace: View {
  let name: String

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      Image(systemName: "doc.text")
        .symbolStyle(.body)
        .foregroundStyle(Color(.textTertiary))
      Spacer(minLength: 0)
      Text(name)
        // The mockup's 10 px label is closest to `font.micro`.
        .textStyle(.micro)
        .lineLimit(1)
        .truncationMode(.middle)
        .foregroundStyle(Color(.textSecondary))
    }
    .padding(.horizontal, Space.s)
    .padding(.vertical, Space.xs)
    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
    .background(Color(.fillPrimary))
  }
}

#Preview("image") {
  PreviewMatrix { Thumbnail(PreviewState.imageName, image: PreviewState.screenshot) }
}
#Preview("file") { PreviewMatrix { Thumbnail(PreviewState.fileName) } }
