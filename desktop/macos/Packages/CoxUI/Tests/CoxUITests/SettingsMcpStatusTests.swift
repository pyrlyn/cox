// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The MCP page's status badges and log (T37.45.4, DT§5.7, mockup 20): connected, needs login,
// failed with Show log, and unknown on one page in every light/dark × Solid/Frosted cell; the
// page with MCP off; and the log sheet a failed server opens.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct SettingsMcpStatusSnapshotTests {
  @Test(arguments: Variant.all) func eachServerShowsItsStatusBadge(_ variant: Variant) throws {
    try check(PreviewState.settingsMcpStatuses, variant)
  }

  @Test func withMcpOffEveryServerShowsDisabled() throws {
    try check(PreviewState.settingsMcpDisabled, Variant.all[0])
  }

  @Test(arguments: [Variant.all[0], Variant.all[3]])
  func showLogOpensTheServersLogSheet(_ variant: Variant) throws {
    try assertCoxSnapshot(
      TaskOutputSheet(
        title: "sentry log", output: PreviewState.mcpLog.joined(separator: "\n")
      ) {}, variant, named: variant.name)
  }

  private func check(
    _ state: SettingsScreenState, _ variant: Variant, test: String = #function
  ) throws {
    try assertCoxWindowSnapshot(
      SettingsScreen(state: state) { _ in }, variant,
      size: CGSize(width: Size.windowMinWidth, height: Size.windowMinHeight), testName: test)
  }
}
