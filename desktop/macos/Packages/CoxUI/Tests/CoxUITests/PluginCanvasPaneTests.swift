// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PluginCanvasPane`'s check (T33.47.8.4, plugin-canvas §3): snapshots of the pane with two
// frames, and with the picker, stopped and zoomed out, × light/dark × Solid/Frosted; the zoom's
// steps and clamp; and the toggle's key.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct PluginCanvasPaneSnapshotTests {
  @Test(arguments: Variant.all) func canvasPane(_ variant: Variant) throws {
    try check(PreviewState.canvasTwoFrames, variant, "two-frames")
    try check(PreviewState.canvasPicker, variant, "picker")
  }

  private func check(
    _ state: PluginCanvasPaneState, _ variant: Variant, _ look: String, test: String = #function
  ) throws {
    try assertCoxSnapshot(
      PreviewPane { PluginCanvasSample(state: state) }, variant, named: "\(look).\(variant.name)",
      testName: test)
  }
}

@Suite struct PluginCanvasZoomTests {
  @Test func theZoomStepsOnAQuarterGrid() {
    #expect(PluginCanvasPaneState.zoomed(1, in: true) == 1.25)
    #expect(PluginCanvasPaneState.zoomed(1, in: false) == 0.75)
    // A pinch leaves the zoom off the grid; the next step lands back on it.
    #expect(PluginCanvasPaneState.zoomed(1.1, in: true) == 1.25)
  }

  @Test func theZoomIsClamped() {
    #expect(PluginCanvasPaneState.zoomed(2, in: true) == 2)
    #expect(PluginCanvasPaneState.zoomed(0.25, in: false) == 0.25)
    #expect(PluginCanvasPaneState.clamped(9) == 2)
    #expect(PluginCanvasPaneState(canvases: [], shown: "x", zoom: 0.01).zoom == 0.25)
  }
}

@MainActor
@Suite struct CanvasShortcutTests {
  @Test func theCanvasTogglesOnCommandShiftD() {
    #expect(ShellShortcut.canvas.key == KeyboardShortcut("d", modifiers: [.command, .shift]))
    #expect(ShellShortcut.canvas.help("Show canvas") == "Show canvas (⌘⇧D)")
  }
}
