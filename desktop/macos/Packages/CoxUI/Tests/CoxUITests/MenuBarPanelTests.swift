// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `MenuBarPanel`'s check (T51.13, DS§6.4, mockup 26): snapshots of the panel empty and with an
// approval, a question and a running session × light/dark × Solid/Frosted, on a pane as its
// `#Preview` shows it.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct MenuBarPanelSnapshotTests {
  @Test(arguments: Variant.all) func menuBarPanel(_ variant: Variant) throws {
    try check(PreviewState.menuBarEmpty, variant, "empty")
    try check(PreviewState.menuBarBusy, variant, "busy")
  }

  /// One image per state, named `<state>.<cell>`.
  private func check(
    _ state: MenuBarPanel.State, _ variant: Variant, _ look: String, test: String = #function
  ) throws {
    try assertCoxSnapshot(
      PreviewPane { MenuBarPanel(state: state) { _ in }.fixedSize() }, variant,
      named: "\(look).\(variant.name)", testName: test)
  }
}
