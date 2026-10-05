// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// T52.17's CoxUI check (DS§6.4, PL§8): the plugin panel above the composer and the toolbar's
// plugin status segments, each in light/dark × Solid/Frosted.

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
}
