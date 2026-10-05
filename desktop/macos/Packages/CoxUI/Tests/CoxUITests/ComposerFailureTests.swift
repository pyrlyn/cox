// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The composer's failure notice (T37.24.9, DS§6.4 row `Composer`): why the last send failed
// stands above the composer as an error `NoticeRow`, the draft still in it, × light/dark ×
// Solid/Frosted.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct ComposerFailureTests {
  @Test(arguments: Variant.all) func failure(_ variant: Variant) throws {
    try assertCoxSnapshot(
      ComposerSample(state: PreviewState.composerFailure).fixedSize(), variant,
      named: "failure.\(variant.name)")
  }
}
