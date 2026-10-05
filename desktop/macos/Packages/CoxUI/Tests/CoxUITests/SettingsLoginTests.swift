// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The MCP page's logins (T37.30.3, DT§5.7): a fixture server logged out with Log in offered,
// then logged in with Log out offered after the scripted callback, beside a stdio server with
// no login; the logged-out page in every light/dark × Solid/Frosted cell.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct SettingsLoginSnapshotTests {
  @Test(arguments: Variant.all) func aServerShowsLoggedOutWithLogIn(_ variant: Variant) throws {
    try check(PreviewState.settingsMcp, variant)
  }

  @Test func afterTheCallbackItShowsLoggedInWithLogOut() throws {
    try check(PreviewState.settingsMcpLoggedIn, Variant.all[0])
  }

  private func check(
    _ state: SettingsScreenState, _ variant: Variant, test: String = #function
  ) throws {
    try assertCoxWindowSnapshot(
      SettingsScreen(state: state) { _ in }, variant,
      size: CGSize(width: Size.windowMinWidth, height: Size.windowMinHeight), testName: test)
  }
}
