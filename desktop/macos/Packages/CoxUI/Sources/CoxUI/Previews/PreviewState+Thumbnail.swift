// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState`'s attachment fixtures (DS§5, DS§9) for `Thumbnail`'s previews and snapshots.
// Separate from `PreviewState.swift` so the atoms added alongside it each keep their fixtures
// in their own file.

import SwiftUI

extension PreviewState {
  static let imageName = "window.png"
  static let fileName = "provider-rate-limit-trace.log"

  /// A window-sized picture of the preview backdrop, as a pasted screenshot would be.
  @MainActor static let screenshot = renderScreenshot()

  @MainActor private static func renderScreenshot() -> Image {
    let renderer = ImageRenderer(
      content: PreviewBackdrop().frame(width: Size.windowMinWidth, height: Size.windowMinHeight))
    return renderer.nsImage.map { Image(nsImage: $0) } ?? Image(systemName: "photo")
  }
}
