// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ApprovalCard` and `QuestionCard`'s check (T37.27, DS§6.4): a snapshot per state × light/dark ×
// Solid/Frosted — a pending approval, a subagent's risky one, one with its grant and Edit…
// (T37.27.6), allowed and denied lines, a
// question with options, one with only the field, and an answered one — each on a pane as its
// `#Preview` shows it.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct ApprovalCardSnapshotTests {
  @Test(arguments: Variant.all) func approvalCard(_ variant: Variant) throws {
    try check(ApprovalCardSample(PreviewState.approvalPending), variant, "pending")
    try check(ApprovalCardSample(PreviewState.approvalRisky), variant, "risky")
    try check(ApprovalCardSample(PreviewState.approvalGrant), variant, "grant")
    try check(ApprovalCardSample(PreviewState.approvalAllowed), variant, "allowed")
    try check(ApprovalCardSample(PreviewState.approvalDenied), variant, "denied")
  }

  @Test(arguments: Variant.all) func questionCard(_ variant: Variant) throws {
    try check(QuestionCardSample(PreviewState.questionOptions), variant, "options")
    try check(QuestionCardSample(PreviewState.questionOpen), variant, "open")
    try check(QuestionCardSample(PreviewState.questionAnswered), variant, "answered")
  }

  @Test func anEditIsSentOnlyAsTrimmedJSON() {
    #expect(EditedInput(" {\"command\": \"ls\"}\n").json == #"{"command": "ls"}"#)
    #expect(EditedInput(#"{"command": "ls""#).json == nil)
    #expect(EditedInput(nil).json == nil)
  }

  /// One image per state, named `<state>.<cell>`, at its ideal size.
  private func check(
    _ card: some View, _ variant: Variant, _ look: String, test: String = #function
  ) throws {
    try assertCoxSnapshot(
      PreviewPane { card.fixedSize(horizontal: false, vertical: true) }, variant,
      named: "\(look).\(variant.name)", testName: test)
  }
}
