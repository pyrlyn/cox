// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Mockup 18's provider keys and pop-ups (T37.45.2, DT§5.7): the Models & Providers page with one
// key stored and one missing, the key row in each state, its sheet with an empty secure field,
// and a pop-up, in every light/dark × Solid/Frosted cell.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct SettingsKeysSnapshotTests {
  @Test(arguments: Variant.all)
  func providersPageShowsKeyStatesAndPopUps(_ variant: Variant) throws {
    try assertCoxWindowSnapshot(
      SettingsScreen(state: PreviewState.settingsProviders) { _ in }, variant,
      size: CGSize(width: Size.windowMinWidth, height: Size.windowMinHeight))
  }

  @Test(arguments: Variant.all) func keyRowSaysWhetherAKeyIsStored(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane {
        SettingsGroupBox("providers") {
          KeyRowSample.stored
          KeyRowSample.none
        }
        .frame(width: Size.readingWidth)
      }, variant, named: variant.name)
  }

  @Test(arguments: Variant.all) func keySheetStartsEmpty(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane { KeySheet(provider: "anthropic", isStored: true) { _ in } }, variant,
      named: variant.name)
  }

  @Test(arguments: Variant.all) func settingPopUpShowsTheSelection(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane {
        SettingPopUp(
          "Effort", selection: .constant("high"), options: PreviewState.popUpOptions,
          title: { $0 })
      }, variant, named: variant.name)
  }
}
