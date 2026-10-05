// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The composer molecules' check (T37.21.7, DS§6.3): a snapshot per variant × light/dark ×
// Solid/Frosted, each molecule on a pane from `PreviewState` as its `#Preview` shows it.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct ComposerMoleculeSnapshotTests {
  @Test(arguments: Variant.all) func composerChip(_ variant: Variant) throws {
    for kind in ComposerChip.Kind.allCases {
      try check(ComposerChipSample(kind: kind), variant, kind.name)
    }
    try check(ComposerChipSample.shortcut, variant, "shortcut")
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

extension ComposerChip.Kind {
  /// The snapshot's name: the case, a mode's mode and whether think is on.
  fileprivate var name: String {
    switch self {
    case .mode(let mode): "mode-\(mode)"
    case .think(let isOn): isOn ? "think-on" : "think-off"
    default: "\(self)"
    }
  }
}
