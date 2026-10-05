// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Permissions page's rules and session grants (T37.45.3, mockup 19): rules from three
// layers with the project's locked and the add row, a grant with Revoke, in every
// light/dark × Solid/Frosted cell; then the grammar's refusal under the add row.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct SettingsPermissionsSnapshotTests {
  @Test(arguments: Variant.all) func theRulesShowTheirLayerAndTheGrantItsRevoke(
    _ variant: Variant
  ) throws {
    try check(PreviewState.settingsPermissionRules, variant)
  }

  @Test func aRefusedRuleShowsTheGrammarsMessage() throws {
    try check(PreviewState.settingsPermissionRulesRefused, Variant.all[0])
  }

  @Test func noGrantsShowsTheEmptyRow() throws {
    var state = PreviewState.settingsPermissionRules
    state.permissions?.grants = []
    try check(state, Variant.all[0])
  }

  private func check(
    _ state: SettingsScreenState, _ variant: Variant, test: String = #function
  ) throws {
    try assertCoxWindowSnapshot(
      SettingsScreen(state: state) { _ in }, variant,
      size: CGSize(width: Size.windowMinWidth, height: Size.windowMinHeight), testName: test)
  }
}
