// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `CommandPalette`'s check (T37.44.13, DS§6.4, mockup 12): snapshots of the palette with mockup
// 12's `rev`, as ⌘K opens it, and with nothing matched × light/dark × Solid/Frosted, on a pane as
// its `#Preview` shows it.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct CommandPaletteSnapshotTests {
  @Test(arguments: Variant.all) func commandPalette(_ variant: Variant) throws {
    try check(PreviewState.paletteReview, variant, "review")
    try check(PreviewState.paletteOpen, variant, "open")
    try check(PreviewState.paletteNoMatch, variant, "no-match")
  }

  /// One image per state, named `<state>.<cell>`.
  private func check(
    _ state: CommandPalette.State, _ variant: Variant, _ look: String, test: String = #function
  ) throws {
    try assertCoxSnapshot(
      PreviewPane { CommandPalette(state: state) { _ in }.fixedSize() }, variant,
      named: "\(look).\(variant.name)", testName: test)
  }
}
