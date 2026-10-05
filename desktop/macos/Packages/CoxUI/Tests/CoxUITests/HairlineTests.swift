// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The `Hairline` atom's check (T37.20.4, DS§6.2): a snapshot per orientation × light/dark ×
// Solid/Frosted, on a pane from `PreviewState` as its `#Preview` shows it. The `.hairline`
// modifier keeps its own images under `FoundationsTests`.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct HairlineTests {
  @Test(arguments: Variant.all) func hairlineAtom(_ variant: Variant) throws {
    try check(Hairline(.horizontal).frame(width: Size.popoverWidth), variant, "horizontal")
    try check(Hairline(.vertical).frame(height: Size.capsuleHeight), variant, "vertical")
  }

  private func check(
    _ rule: some View, _ variant: Variant, _ look: String, test: String = #function
  ) throws {
    try assertCoxSnapshot(
      PreviewPane { rule }, variant, named: "\(look).\(variant.name)", testName: test)
  }
}
