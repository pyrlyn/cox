// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The atoms' check (T37.20, DS§6.2): a snapshot per variant × light/dark × Solid/Frosted, each
// atom on a pane from `PreviewState` as its `#Preview` shows it, and the mappings the atoms own.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct AtomSnapshotTests {
  @Test(arguments: Variant.all) func statusDot(_ variant: Variant) throws {
    for status in StatusDot.Status.allCases {
      try check(StatusDot(status), variant, "\(status)")
    }
  }

  @Test(arguments: Variant.all) func iconTile(_ variant: Variant) throws {
    for kind in IconTile.Kind.allCases {
      try check(IconTile(kind, symbol: PreviewState.symbol(kind)), variant, "\(kind)")
    }
  }

  @Test(arguments: Variant.all) func keyCap(_ variant: Variant) throws {
    try check(KeyCap(PreviewState.keys), variant)
  }

  @Test(arguments: Variant.all) func badge(_ variant: Variant) throws {
    for kind in Badge.Kind.allCases {
      try check(Badge(PreviewState.badge(kind), kind: kind), variant, "\(kind)")
    }
  }

  @Test(arguments: Variant.all) func countBadge(_ variant: Variant) throws {
    try check(CountBadge(PreviewState.count), variant)
  }

  @Test(arguments: Variant.all) func riskChip(_ variant: Variant) throws {
    for level in RiskChip.Level.allCases {
      try check(RiskChip(PreviewState.risk, level: level), variant, "\(level)")
    }
  }

  @Test(arguments: Variant.all) func diffStat(_ variant: Variant) throws {
    try check(DiffStat(added: PreviewState.added, removed: PreviewState.removed), variant)
  }

  @Test(arguments: Variant.all) func sectionHeader(_ variant: Variant) throws {
    try check(
      SectionHeader(PreviewState.sectionTitle).frame(width: Size.sidebarWidth), variant, "title")
    try check(
      SectionHeader(PreviewState.sectionTitle) { CountBadge(PreviewState.count) }
        .frame(width: Size.sidebarWidth),
      variant, "trailing")
  }

  @Test(arguments: Variant.all) func inlineCode(_ variant: Variant) throws {
    try check(InlineCode(PreviewState.inlineCode), variant)
  }

  /// One image per atom variant, named `<variant>.<cell>`, or `<cell>` for a one-look atom.
  private func check(
    _ atom: some View, _ variant: Variant, _ look: String? = nil, test: String = #function
  ) throws {
    let name = [look, variant.name].compactMap(\.self).joined(separator: ".")
    try assertCoxSnapshot(PreviewPane { atom }, variant, named: name, testName: test)
  }
}

@MainActor
@Suite struct AtomTests {
  @Test func onlyALiveStatusGlows() {
    let glowing = StatusDot.Status.allCases.filter { $0.halo != nil }
    #expect(glowing == [.running, .waiting])
  }

  @Test func eachRiskLevelDrawsInItsOwnRiskRole() {
    let levels = RiskChip.Level.allCases
    #expect(levels.map(\.foreground) == [Color(.riskLow), Color(.riskMedium), Color(.riskHigh)])
    #expect(Set(levels.map(\.background)).count == levels.count)
  }

  @Test func diffStatReadsAsLinesAddedAndRemoved() {
    #expect(DiffStat(added: 42, removed: 7).label == "42 lines added, 7 removed")
  }

  @Test func anAddOnlyDiffStatShowsNoZeroRemoved() {
    let stat = DiffStat(added: 12, removed: 0)
    #expect(!stat.showsRemoved)
    #expect(stat.label == "12 lines added")
  }
}
