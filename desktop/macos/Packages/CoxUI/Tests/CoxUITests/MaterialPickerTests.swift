// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The material picker's check (T37.21.8, DS§6.3): a snapshot per selected material and at Flat
// Depth × light/dark × Solid/Frosted, on a pane from `PreviewState` as its `#Preview` shows it,
// and the choices the picker owns.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct MaterialPickerSnapshotTests {
  @Test(arguments: Variant.all) func materialPicker(_ variant: Variant) throws {
    for material in MaterialPicker.order {
      try check(MaterialPickerSample(selection: material), variant, material.rawValue)
    }
    try check(MaterialPickerSample(selection: .frosted, depth: 0), variant, "flat")
  }

  /// One image per look, named `<look>.<cell>`, at its ideal size (see
  /// `ShellMoleculeSnapshotTests.check`).
  private func check(
    _ molecule: some View, _ variant: Variant, _ look: String, test: String = #function
  ) throws {
    try assertCoxSnapshot(
      PreviewPane { molecule.fixedSize() }, variant, named: "\(look).\(variant.name)",
      testName: test)
  }
}

@Suite struct MaterialPickerTests {
  @Test func everyMaterialIsOfferedOnceDefaultFirst() {
    #expect(MaterialPicker.order == [.frosted, .glossy, .solid])
    #expect(Set(MaterialPicker.order) == Set(GlassMaterial.allCases))
  }

  @Test func aSwatchPreviewsItsMaterialAtTheUsersDepthAndTextSize() {
    let user = Appearance(material: .solid, windowOpacity: 0.9, depth: 0.25, textScale: 1.2)
    let swatch = user.swatch(.glossy)
    #expect(swatch == Appearance(material: .glossy, depth: 0.25, textScale: 1.2))
    #expect(swatch.windowOpacity == MaterialToken.glossyWindowOpacity)
  }
}
