// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `BrowserPaneChrome`'s check (T51.10, DS§6.4): a snapshot of mockup 25's pane idle, loading
// and over `https` × light/dark × Solid/Frosted, each on a pane as its `#Preview` shows it, and
// the toggle's key.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct BrowserPaneChromeSnapshotTests {
  @Test(arguments: Variant.all) func browserPane(_ variant: Variant) throws {
    try check(PreviewState.browserIdle, variant, "idle")
    try check(PreviewState.browserLoading, variant, "loading")
    try check(PreviewState.browserSecure, variant, "https")
  }

  /// One image per state, named `<state>.<cell>`.
  private func check(
    _ state: BrowserPaneState, _ variant: Variant, _ look: String, test: String = #function
  ) throws {
    try assertCoxSnapshot(
      PreviewPane { BrowserPaneSample(state: state) }, variant, named: "\(look).\(variant.name)",
      testName: test)
  }
}

@MainActor
@Suite struct BrowserShortcutTests {
  @Test func theBrowserTogglesOnCommandShiftB() {
    #expect(ShellShortcut.browser.key == KeyboardShortcut("b", modifiers: [.command, .shift]))
    #expect(ShellShortcut.browser.help("Show browser") == "Show browser (⌘⇧B)")
  }
}
