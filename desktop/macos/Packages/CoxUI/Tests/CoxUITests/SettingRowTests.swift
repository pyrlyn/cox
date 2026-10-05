// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `SettingRow`'s check (T37.21.10, DS§6.3): a snapshot per variant × light/dark × Solid/Frosted
// on a pane from `PreviewState`, and which source layers lock a setting.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct SettingRowSnapshotTests {
  @Test(arguments: Variant.all) func settingRow(_ variant: Variant) throws {
    try check(SettingRowSample.toggle, variant, "toggle")
    try check(SettingRowSample.control, variant, "control")
    try check(SettingRowSample.slider, variant, "slider")
    try check(SettingRowSample.readOnly, variant, "readOnly")
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

@Suite struct SettingRowTests {
  @Test func onlyLayersAboveTheUsersConfigLockASetting() {
    let locked = SettingSource.allCases.filter(\.isReadOnly)
    #expect(locked == [.project, .claudeSettings, .env, .flag])
  }
}
