// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The setting and figure molecules' check (T37.21, DS§6.3): a snapshot per variant ×
// light/dark × Solid/Frosted, each molecule on a pane from `PreviewState` as its `#Preview`
// shows it.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct SettingMoleculeSnapshotTests {
  @Test(arguments: Variant.all) func labeledToggle(_ variant: Variant) throws {
    try check(LabeledToggleSample(isOn: true, detail: nil), variant, "on")
    try check(
      LabeledToggleSample(isOn: false, detail: PreviewState.toggleDetail), variant, "detail")
  }

  @Test(arguments: Variant.all) func labeledSlider(_ variant: Variant) throws {
    try check(LabeledSliderSample(ends: PreviewState.sliderEnds), variant, "ends")
    try check(LabeledSliderSample(ends: nil), variant, "plain")
  }

  @Test(arguments: Variant.all) func keyValueGrid(_ variant: Variant) throws {
    try check(
      KeyValueGrid(columns: PreviewState.tokenColumns, rows: PreviewState.tokenRows)
        .frame(width: Size.tokenPopoverWidth), variant, "columns")
    try check(
      KeyValueGrid(rows: PreviewState.factRows).frame(width: Size.inspectorWidth), variant,
      "pairs")
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
