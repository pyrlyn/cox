// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The tool card's molecules' check (T37.21.1, T37.21.4, DS§6.3): a snapshot per variant ×
// light/dark × Solid/Frosted, each molecule on a pane from `PreviewState` as its `#Preview`
// shows it.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct ToolMoleculeSnapshotTests {
  @Test(arguments: Variant.all) func toolHeader(_ variant: Variant) throws {
    try check(ToolHeaderSample(PreviewState.toolEdited, isExpanded: false), variant, "edited")
    try check(ToolHeaderSample(PreviewState.toolEdited, isExpanded: true), variant, "expanded")
    try check(ToolHeaderSample(PreviewState.toolRunning), variant, "running")
    try check(ToolHeaderSample(PreviewState.toolExplored, isExpanded: false), variant, "explored")
    try check(ToolHeaderSample(PreviewState.toolFailed), variant, "failed")
  }

  @Test(arguments: Variant.all) func terminalTail(_ variant: Variant) throws {
    try check(TerminalTailSample(exit: .running), variant, "running")
    try check(TerminalTailSample(exit: PreviewState.tailSucceeded), variant, "succeeded")
    try check(TerminalTailSample(exit: PreviewState.tailFailed), variant, "failed")
  }

  /// One image per molecule variant, named `<variant>.<cell>`, at its ideal size (see
  /// `ShellMoleculeSnapshotTests`). Reduce Motion holds a running spinner still.
  private func check(
    _ molecule: some View, _ variant: Variant, _ look: String, test: String = #function
  ) throws {
    try assertCoxSnapshot(
      PreviewPane { molecule.fixedSize() }.environment(\._accessibilityReduceMotion, true),
      variant, named: "\(look).\(variant.name)", testName: test)
  }
}
