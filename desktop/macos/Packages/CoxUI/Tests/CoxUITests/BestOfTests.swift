// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// T52.12's CoxUI check (DT§3.3.1, mockup 27): the composer's best-of-n control and the compare
// view with two and three candidates, one of them failed, and both questions "Keep this one"
// asks, each in light/dark × Solid/Frosted; and (T60.10) every candidate failed on the provider.

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

  @Test(arguments: Variant.all) func bestOfNotReady(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane { BestOfControl(state: PreviewState.bestOfNotReady) { _ in } }
        .frame(width: Size.readingWidth), variant, named: "not-ready.\(variant.name)")
  }

  @Test(arguments: Variant.all) func bestOfCompare(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane { BestOfCompare(state: PreviewState.bestOfTwo) { _ in } }, variant,
      named: "two.\(variant.name)")
    try assertCoxSnapshot(
      PreviewPane { BestOfCompare(state: PreviewState.bestOfThree) { _ in } }, variant,
      named: "three.\(variant.name)")
  }

  @Test(arguments: Variant.all) func bestOfFailedTurns(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane { BestOfCompare(state: PreviewState.bestOfFailedTurns) { _ in } }, variant,
      named: variant.name)
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

@MainActor
@Suite struct BestOfMergeSnapshotTests {
  @Test(arguments: Variant.all) func compareOffersMergeOnlyWithMoreThanOneResult(
    _ variant: Variant
  ) throws {
    try assertCoxSnapshot(
      PreviewPane { BestOfCompare(state: PreviewState.bestOfOneResult) { _ in } }, variant,
      named: "one-result.\(variant.name)")
    try assertCoxSnapshot(
      PreviewPane { BestOfCompare(state: PreviewState.bestOfThreeResults) { _ in } }, variant,
      named: "three-results.\(variant.name)")
  }

  @Test(arguments: Variant.all) func mergeSheet(_ variant: Variant) throws {
    for (name, state) in [
      ("two", PreviewState.bestOfMergeTwo), ("one", PreviewState.bestOfMergeOne),
      ("model-menu", PreviewState.bestOfMergeModelMenu),
      ("agent-menu", PreviewState.bestOfMergeAgentMenu),
    ] {
      try assertCoxSnapshot(
        PreviewPane { BestOfMergeSheet(state: state) { _ in } }, variant,
        named: "\(name).\(variant.name)")
    }
  }

  @Test(arguments: Variant.all) func mergeColumn(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane { BestOfCompare(state: PreviewState.bestOfMergeRunning) { _ in } }, variant,
      named: "running.\(variant.name)")
    try assertCoxSnapshot(
      PreviewPane { BestOfCompare(state: PreviewState.bestOfMergeDone) { _ in } }, variant,
      named: "done.\(variant.name)")
  }
}

@Suite struct BestOfTests {
  @Test func bestOfMergeShowsOnlyForMoreThanOneMergeableResult() {
    #expect(!PreviewState.bestOfThree.offersMerge, "a state that lists none offers none")
    #expect(!PreviewState.bestOfOneResult.offersMerge)
    #expect(PreviewState.bestOfThreeResults.offersMerge)
  }

  @Test func bestOfMergeIsBlockedUntilTwoAreCheckedAndAMergerCanStart() {
    #expect(PreviewState.bestOfMergeTwo.canMerge)
    #expect(PreviewState.bestOfMergeTwo.checked == 2)
    #expect(!PreviewState.bestOfMergeOne.canMerge, "one result is not a merge")
    var none = PreviewState.bestOfMergeTwo
    none.merger = ""
    #expect(!none.canMerge, "no merger chosen")
    none.merger = "agent:gemini"
    #expect(!none.canMerge, "a merger that cannot start here cannot merge")
  }

  @Test func bestOfMergeSheetSplitsModelsFromAgents() {
    let mergers = PreviewState.bestOfMergeModelMenu.mergers
    #expect(mergers.filter(\.isAgent).map(\.label) == ["Codex", "Gemini CLI"])
    #expect(mergers.filter { !$0.isAgent }.map(\.label) == ["cox · Sonnet 5", "cox · Opus 5"])
  }

  @Test func bestOfMergeColumnNamesWhatItMerged() {
    let columns = PreviewState.bestOfMergeDone.columns
    #expect(columns.last?.merges == ["Claude Agent", "cox · Sonnet 5"])
    #expect(columns.dropLast().allSatisfy { $0.merges.isEmpty })
  }

  @Test func bestOfCountsTheSessionsOwnAgent() {
    #expect(PreviewState.bestOfControl.count == 2)
    #expect(PreviewState.bestOfControl.canLaunch)
    #expect(!BestOfControl.State(options: [.init(id: "agent:codex", label: "Codex")]).canLaunch)
  }

  @Test func bestOfCannotLaunchWhileTheProviderIsNotReadyHoweverManyAreAdded() {
    let state = PreviewState.bestOfNotReady
    #expect(state.count == 2)
    #expect(!state.canLaunch)
    #expect(state.options.map(\.unavailable).map { $0 != nil } == [false, true])
    var ready = state
    ready.unavailable = nil
    #expect(ready.canLaunch)
  }

  @Test func bestOfFailedCandidateCannotBeKept() {
    let failed = PreviewState.bestOfThree.columns.filter {
      if case .failed = $0.status { true } else { false }
    }
    #expect(failed.map(\.label) == ["Codex"])
    #expect(failed.allSatisfy { !$0.canKeep && !$0.canReview })
  }

  @Test func bestOfFailedTurnsShowTheRealReasonAndNoCostSign() {
    let columns = PreviewState.bestOfFailedTurns.columns
    #expect(columns.allSatisfy { $0.status == .failed("provider error: provider auth failed") })
    #expect(columns.allSatisfy { $0.cost == "$0.00" && !$0.canKeep && !$0.canReview })
  }
}
