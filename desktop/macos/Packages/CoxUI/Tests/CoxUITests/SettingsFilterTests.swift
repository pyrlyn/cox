// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Settings search (T37.45.1, mockups 18–20): the field at the top of the sidebar, empty in
// the page-list snapshots, and here with a query — only the pages and fields it keeps, the match
// bold on `accent.soft` in each label and bold alone on the selected page's `accent` — in every
// light/dark × Solid/Frosted cell.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct SettingsFilterSnapshotTests {
  @Test(arguments: Variant.all) func aQueryMarksItsMatchesInPagesAndLabels(
    _ variant: Variant
  ) throws {
    try assertCoxWindowSnapshot(
      SettingsScreen(state: PreviewState.settingsFiltered) { _ in }, variant,
      size: CGSize(width: Size.windowMinWidth, height: Size.windowMinHeight))
  }
}

@Suite struct SettingsFilterTests {
  @Test func everyMatchIsMarkedIgnoringCase() {
    let marked = AttributedString("Model modes", marking: "MOD", on: nil)
    let bold = marked.runs.filter { $0.inlinePresentationIntent == .stronglyEmphasized }
    #expect(bold.map { String(marked[$0.range].characters) } == ["Mod", "mod"])
  }

  @Test func anEmptyQueryMarksNothing() {
    let marked = AttributedString("Model", marking: " ", on: nil)
    #expect(marked.runs.allSatisfy { $0.inlinePresentationIntent == nil })
  }
}
