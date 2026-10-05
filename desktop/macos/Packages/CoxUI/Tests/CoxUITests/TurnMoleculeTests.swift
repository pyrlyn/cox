// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The turn molecules' check (T37.21.5, T37.21.6, DS§6.3): a snapshot per variant × light/dark
// × Solid/Frosted, each molecule on a pane from `PreviewState` as its `#Preview` shows it.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct TurnMoleculeSnapshotTests {
  @Test(arguments: Variant.all) func userBubble(_ variant: Variant) throws {
    try check(UserBubbleSample(hasAttachments: false), variant, "text")
    try check(UserBubbleSample(hasAttachments: true), variant, "attachments")
    try check(PromptActions { _ in }, variant, "actions")
  }

  @Test(arguments: Variant.all) func thinkingDisclosure(_ variant: Variant) throws {
    try check(ThinkingDisclosureSample(isExpanded: false), variant, "collapsed")
    try check(ThinkingDisclosureSample(isExpanded: true), variant, "expanded")
  }

  @Test(arguments: Variant.all) func noticeRow(_ variant: Variant) throws {
    for kind in NoticeRow.Kind.allCases {
      try check(NoticeRowSample(kind: kind), variant, "\(kind)")
    }
  }

  @Test(arguments: Variant.all) func turnDivider(_ variant: Variant) throws {
    try check(TurnDividerSample(label: PreviewState.dividerLabel), variant, "label")
    try check(TurnDividerSample(label: nil), variant, "bare")
  }

  @Test(arguments: Variant.all) func turnMeta(_ variant: Variant) throws {
    try check(TurnMeta(PreviewState.turnMeta), variant, "full")
    try check(TurnMeta(PreviewState.turnMetaShort), variant, "short")
  }

  /// One image per molecule variant, named `<variant>.<cell>`, at its ideal size (see
  /// `ShellMoleculeSnapshotTests.check`).
  private func check(
    _ molecule: some View, _ variant: Variant, _ look: String, test: String = #function
  ) throws {
    try assertCoxSnapshot(
      PreviewPane { molecule.fixedSize() }, variant, named: "\(look).\(variant.name)",
      testName: test)
  }
}

@Suite struct TurnMoleculeTests {
  @Test func turnMetaShowsOnlyTheFactsItHasInTheMockupOrder() {
    #expect(
      PreviewState.turnMeta.parts == [
        "Sonnet 5", "in 48.2k · out 3.1k", "cache 91%", "$0.44", "2 m 18 s", "end turn",
      ])
    #expect(PreviewState.turnMetaShort.parts == ["in 2.4k · out 310", "$0.01", "4 s"])
  }
}
