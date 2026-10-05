// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `Thumbnail`'s check (T37.20.3, DS§6.2): a snapshot of the image and the file variant ×
// light/dark × Solid/Frosted, on a pane from `PreviewState` as its `#Preview` shows it.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct ThumbnailTests {
  @Test(arguments: Variant.all) func thumbnail(_ variant: Variant) throws {
    try check(Thumbnail(PreviewState.imageName, image: PreviewState.screenshot), variant, "image")
    try check(Thumbnail(PreviewState.fileName), variant, "file")
  }

  private func check(
    _ thumbnail: Thumbnail, _ variant: Variant, _ look: String, test: String = #function
  ) throws {
    try assertCoxSnapshot(
      PreviewPane { thumbnail }, variant, named: "\(look).\(variant.name)", testName: test)
  }
}
