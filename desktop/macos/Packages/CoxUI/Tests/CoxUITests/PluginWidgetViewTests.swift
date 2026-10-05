// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PluginWidgetView`'s check (T52.16, DS§6.3, PL§8): a snapshot per widget variant and a nested
// tree in light/dark × Solid/Frosted, the nested tree again under Increase Contrast, and the
// span's own mapping from a plugin's role to a token.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct PluginWidgetSnapshotTests {
  @Test(arguments: Variant.all) func eachVariant(_ variant: Variant) throws {
    for sample in PreviewState.pluginWidgets {
      try check(PluginWidgetView(sample.widget), variant, sample.name)
    }
  }

  @Test(arguments: Variant.all) func nestedUnderIncreasedContrast(_ variant: Variant) throws {
    let nested = PluginWidgetView(PreviewState.pluginBlock)
    try check(nested.environment(\._colorSchemeContrast, .increased), variant, "increased")
  }

  /// One image per widget, named `<widget>.<variant>`, at the width of a popover so a panel's
  /// columns have room.
  private func check(
    _ widget: some View, _ variant: Variant, _ look: String, test: String = #function
  ) throws {
    try assertCoxSnapshot(
      PreviewPane { widget.frame(width: Size.popoverWidth) }, variant,
      named: "\(look).\(variant.name)", testName: test)
  }
}

@Suite struct PluginWidgetTests {
  @Test func aSpanCarriesItsRoleColourAndEmphasis() {
    let line = PluginSpan.attributed([.init("slow", .warn, isBold: true, isItalic: true)])
    let run = line.runs.first
    #expect(run?.foregroundColor == Color(.statusWarning))
    #expect(run?.inlinePresentationIntent == [.stronglyEmphasized, .emphasized])
  }

  @Test func noRoleWearsThePlanModeTint() {
    #expect(!PluginSpan.Role.allCases.contains { $0.colour == Color(.statusPlan) })
  }
}
