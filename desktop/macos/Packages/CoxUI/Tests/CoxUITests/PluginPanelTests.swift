// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// T52.17's CoxUI check (DS§6.4, PL§8): the plugin panel above the composer and the toolbar's
// plugin status segments, and T33.45.7's inspector tab and toolbar items (PL§15.4), each in
// light/dark × Solid/Frosted; and the symbol fallback those draw with.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct PluginPanelSnapshotTests {
  @Test(arguments: Variant.all) func panel(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane { PluginPanel(PreviewState.pluginPanels).frame(width: Size.readingWidth) },
      variant, named: "panel.\(variant.name)")
  }

  @Test(arguments: Variant.all) func statusSegment(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane { PluginStatusSegments(widgets: PreviewState.pluginStatus).fixedSize() },
      variant, named: "status.\(variant.name)")
  }

  @Test(arguments: Variant.all) func inspectorTab(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane { PreviewState.pluginInspector.frame(height: Size.inspectorWidth) },
      variant, named: "inspector.\(variant.name)")
  }

  @Test(arguments: Variant.all) func toolbarItems(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane { SessionToolbar(state: PreviewState.pluginToolbar) { _ in }.fixedSize() },
      variant, named: "toolbar.\(variant.name)")
  }
}

@Suite struct PluginSymbolTests {
  /// Settles PL§15.4's unverified note: the system answers nil for a name it does not ship.
  @Test func anUnknownSymbolDrawsTheExtensionGlyph() {
    #expect(PluginSymbol.name("chart.bar") == "chart.bar")
    #expect(PluginSymbol.name("not.a.symbol") == "puzzlepiece.extension")
    #expect(PluginSymbol.name(nil) == "puzzlepiece.extension")
  }
}
