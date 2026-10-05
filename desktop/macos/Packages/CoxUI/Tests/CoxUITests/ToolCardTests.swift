// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ToolCard`'s check (T37.23, DS§6.4): a snapshot per state × light/dark × Solid/Frosted — a
// folded and an opened edit, a running command with its tail, an opened failure and a call with
// nothing to open — each on a pane as its `#Preview` shows it.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct ToolCardSnapshotTests {
  @Test(arguments: Variant.all) func toolCard(_ variant: Variant) throws {
    try check(ToolCardSample(PreviewState.cardEdited), variant, "edited")
    try check(ToolCardSample(PreviewState.cardEdited, isExpanded: true), variant, "edited-open")
    try check(ToolCardSample(PreviewState.cardRunning), variant, "running")
    try check(ToolCardSample(PreviewState.cardFailed, isExpanded: true), variant, "failed-open")
    try check(ToolCardSample(PreviewState.cardExplored), variant, "explored")
  }

  @Test(arguments: Variant.all) func pluginRenderer(_ variant: Variant) throws {
    try check(ToolCardSample(PreviewState.cardPlugin, isExpanded: true), variant, "plugin")
  }

  /// One image per state, named `<state>.<cell>`, at its ideal size. Reduce Motion holds a
  /// running spinner still.
  private func check(
    _ card: some View, _ variant: Variant, _ look: String, test: String = #function
  ) throws {
    try assertCoxSnapshot(
      PreviewPane { card.fixedSize() }.environment(\._accessibilityReduceMotion, true),
      variant, named: "\(look).\(variant.name)", testName: test)
  }
}
