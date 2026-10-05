// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `DecisionBar`'s check (T37.27.5, DS§6.4): a snapshot per state × light/dark × Solid/Frosted —
// a pending approval, a question with its answers on the line, and one whose answers do not fit
// — each on a pane as its `#Preview` shows it.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct DecisionBarSnapshotTests {
  @Test(arguments: Variant.all) func decisionBar(_ variant: Variant) throws {
    try check(PreviewState.decisionApproval, variant, "approval")
    try check(PreviewState.decisionQuestion, variant, "question")
    try check(PreviewState.decisionLongQuestion, variant, "long-question")
  }

  /// One image per state, named `<state>.<cell>`, at its ideal height.
  private func check(
    _ content: DecisionBar.Content, _ variant: Variant, _ look: String, test: String = #function
  ) throws {
    try assertCoxSnapshot(
      PreviewPane { DecisionBarSample(content).fixedSize(horizontal: false, vertical: true) },
      variant, named: "\(look).\(variant.name)", testName: test)
  }
}
