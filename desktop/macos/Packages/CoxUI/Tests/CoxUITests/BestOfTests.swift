// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// T52.12's CoxUI check (DT§3.3.1, mockup 27): the composer's best-of-n control and the compare
// view with two and three candidates, one of them failed, and both questions "Keep this one"
// asks, each in light/dark × Solid/Frosted.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct BestOfSnapshotTests {
  @Test(arguments: Variant.all) func bestOfControl(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane { BestOfControl(state: PreviewState.bestOfControl) { _ in } }
        .frame(width: Size.readingWidth), variant, named: variant.name)
  }

  @Test(arguments: Variant.all) func bestOfCompare(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane { BestOfCompare(state: PreviewState.bestOfTwo) { _ in } }, variant,
      named: "two.\(variant.name)")
    try assertCoxSnapshot(
      PreviewPane { BestOfCompare(state: PreviewState.bestOfThree) { _ in } }, variant,
      named: "three.\(variant.name)")
  }

  @Test(arguments: Variant.all) func bestOfKeep(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane { BestOfCompare(state: PreviewState.bestOfConfirm) { _ in } }, variant,
      named: "confirm.\(variant.name)")
    try assertCoxSnapshot(
      PreviewPane { BestOfCompare(state: PreviewState.bestOfDirty) { _ in } }, variant,
      named: "dirty.\(variant.name)")
  }
}

@Suite struct BestOfTests {
  @Test func bestOfCountsTheSessionsOwnAgent() {
    #expect(PreviewState.bestOfControl.count == 2)
    #expect(PreviewState.bestOfControl.canLaunch)
    #expect(!BestOfControl.State(options: [.init(id: "agent:codex", label: "Codex")]).canLaunch)
  }

  @Test func bestOfFailedCandidateCannotBeKept() {
    let failed = PreviewState.bestOfThree.columns.filter {
      if case .failed = $0.status { true } else { false }
    }
    #expect(failed.map(\.label) == ["Codex"])
    #expect(failed.allSatisfy { !$0.canKeep && !$0.canReview })
  }
}
