// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `TerminalPaneChrome`'s check (T51.6, DS§6.4): a snapshot of mockup 24's one tab and of two
// tabs × light/dark × Solid/Frosted, each on a pane as its `#Preview` shows it, and the toggle's
// key.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct TerminalPaneChromeSnapshotTests {
  @Test(arguments: Variant.all) func terminalPane(_ variant: Variant) throws {
    try check(PreviewState.terminalOneTab, variant, "one-tab")
    try check(PreviewState.terminalTwoTabs, variant, "two-tabs")
  }

  /// One image per state, named `<state>.<cell>`.
  private func check(
    _ state: TerminalPaneState, _ variant: Variant, _ look: String, test: String = #function
  ) throws {
    try assertCoxSnapshot(
      PreviewPane { TerminalPaneSample(state: state) }, variant, named: "\(look).\(variant.name)",
      testName: test)
  }
}

@MainActor
@Suite struct TerminalShortcutTests {
  @Test func theTerminalTogglesOnControlBacktick() {
    #expect(ShellShortcut.terminal.key == KeyboardShortcut("`", modifiers: .control))
    #expect(ShellShortcut.terminal.help("Show terminal") == "Show terminal (⌃`)")
  }
}
