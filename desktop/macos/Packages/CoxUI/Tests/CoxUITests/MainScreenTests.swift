// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The window shell's check (T37.22, DS§4, DS§6.4): the main screen in Solid, Frosted and
// Glossy, light and dark, laid out as mockup screens 28–29 — sidebar, toolbar, transcript
// column and inspector as floating panes — and with both side panes folded; its organisms per
// light/dark × Solid/Frosted cell; the pane table the shell is built from; and, below 1280 pt,
// the inspector floating over a column that keeps its width (T37.22.1); the Bypass strip and
// the shell toggles' keys and tooltips (T37.22.2).

import SwiftUI
import Testing

@testable import CoxUI

extension Variant {
  /// The DS§9 matrix plus Glossy, the card's third material.
  static let withGlossy =
    all + [ColorScheme.light, .dark].map { Variant(scheme: $0, material: .glossy) }
}

@MainActor
@Suite struct MainScreenSnapshotTests {
  @Test(arguments: Variant.withGlossy) func mainScreen(_ variant: Variant) throws {
    try checkWindow(MainScreenSample(state: PreviewState.main), variant)
  }

  @Test func mainScreenWithPanesFolded() throws {
    try checkWindow(MainScreenSample(state: PreviewState.folded), Variant.all[1])
  }

  /// At the minimum width, below `MainScreen.inspectorFloatsBelow`, the inspector floats over
  /// the column; `mainScreen` above is the 1440 pt window, where it takes its own width.
  @Test func mainScreenWithTheInspectorFloating() throws {
    try assertCoxWindowSnapshot(
      MainScreenSample(state: PreviewState.main), Variant.all[1], size: Self.narrow)
  }

  /// Bypass is on: the red strip runs under the toolbar (A89), with the sidebar out and in.
  @Test func mainScreenInBypass() throws {
    var state = PreviewState.main
    state.toolbar.mode = .bypass
    try checkWindow(MainScreenSample(state: state), Variant.all[1])
    state.isSidebarVisible = false
    try assertCoxWindowSnapshot(MainScreenSample(state: state), Variant.all[3], named: "folded")
  }

  @Test(arguments: Variant.all) func sidebar(_ variant: Variant) throws {
    try assertCoxSnapshot(
      Sidebar(state: PreviewState.sidebar) { _ in }.frame(height: Size.windowMinHeight), variant,
      named: variant.name)
  }

  /// The folded main screen shows the bar with the sidebar toggle it gains.
  @Test(arguments: Variant.all) func sessionToolbar(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane { SessionToolbar(state: PreviewState.toolbar) { _ in }.fixedSize() }, variant,
      named: variant.name)
  }

  @Test(arguments: Variant.all) func inspectorFrame(_ variant: Variant) throws {
    try assertCoxSnapshot(
      Inspector(selection: .changes, content: EmptyView()) { _ in }
        .frame(height: Size.toolbarHeight * 2), variant, named: variant.name)
  }

  /// The minimum window, 1100 pt wide.
  static let narrow = CGSize(width: Size.windowMinWidth, height: PreviewState.window.height)

  private func checkWindow(
    _ screen: some View, _ variant: Variant, test: String = #function
  ) throws {
    try assertCoxWindowSnapshot(screen, variant, testName: test)
  }
}

/// The shell toggles answer the system commands' default keys and name them in their tooltips.
@MainActor
@Suite struct ShellShortcutTests {
  @Test func theTogglesUseTheSystemCommandsDefaultKeys() {
    // SidebarCommands and InspectorCommands put Show Sidebar on ⌃⌘S and Show Inspector on
    // ⌃⌘I (macOS 27 SDK; Apple's InspectorCommands page names Control-Command-I).
    #expect(ShellShortcut.sidebar.key == KeyboardShortcut("s", modifiers: [.control, .command]))
    #expect(ShellShortcut.inspector.key == KeyboardShortcut("i", modifiers: [.control, .command]))
    #expect(ShellShortcut.appearance.key == KeyboardShortcut("a", modifiers: [.command, .option]))
  }

  /// The toolbar and sidebar buttons take their tooltips from `help(_:)`. A hosted view shows
  /// neither an `NSView.toolTip` nor, with no assistive client, an accessibility tree to read
  /// them back from, so the strings are checked here.
  @Test func theTooltipsNameTheShortcuts() {
    #expect(ShellShortcut.sidebar.help("Show sidebar") == "Show sidebar (⌃⌘S)")
    #expect(ShellShortcut.inspector.help("Hide inspector") == "Hide inspector (⌃⌘I)")
    #expect(ShellShortcut.appearance.help("Appearance") == "Appearance (⌘⌥A)")
  }
}

/// How wide the transcript column is laid out, read from inside its slot.
@MainActor
@Suite struct InspectorOverlayTests {
  @Test func theColumnKeepsItsWidthWhenTheInspectorFloats() throws {
    let narrow = MainScreenSnapshotTests.narrow.width
    let shown = try columnWidth(window: narrow, inspector: true)
    #expect(shown > 0)
    #expect(shown == (try columnWidth(window: narrow, inspector: false)))
  }

  /// The same measure sees the inspector take its width on the 1440 pt window, so the claim
  /// above cannot pass by measuring nothing.
  @Test func aWideWindowGivesTheInspectorItsOwnWidth() throws {
    let wide = PreviewState.window.width
    let hidden = try columnWidth(window: wide, inspector: false)
    #expect(
      try columnWidth(window: wide, inspector: true)
        == hidden - Size.inspectorWidth - Size.paneGap)
  }

  private func columnWidth(window width: CGFloat, inspector: Bool) throws -> CGFloat {
    /// Records the width it is laid out at while it draws nothing.
    final class Probe {
      var width: CGFloat = 0
      func clear(at size: CGSize) -> Color {
        width = size.width
        return .clear
      }
    }
    let probe = Probe()
    var state = PreviewState.main
    state.isInspectorVisible = inspector
    let screen = MainScreen(
      state: state, send: { _ in },
      transcript: {
        GeometryReader { probe.clear(at: $0.size) }
      }, inspector: { _ in })
    _ = try SnapshotHost(
      screen.frame(width: width, height: PreviewState.window.height), Variant.all[0]
    ).bitmap()
    return probe.width
  }
}

@Suite struct MainScreenTests {
  @Test func paneKindsFollowTheDepthAndRadiusScales() {
    let kinds = ShellPaneKind.allCases
    #expect(kinds.map(\.elevation) == [.e5, .e2, .e0, .e2])
    #expect(kinds.map(\.radius) == [Radius.window, Radius.panel, Radius.pane, Radius.pane])
  }

  /// T51.23: on glass the panes draw `glass.fill` as the token has it, rimmed by `glass.border`;
  /// the window keeps its `surface.window` tint, and Solid keeps every plain surface.
  @Test func panesOnGlassDrawTheGlassTokens() {
    let panes = [ShellPaneKind.sidebar, .column, .inspector]
    for material in [GlassMaterial.frosted, .glossy] {
      let glass = Appearance(material: material)
      #expect(panes.allSatisfy { $0.fill(glass) == Color(.glassFill) && $0.role(glass) == .tint })
      #expect(ShellPaneKind.allCases.allSatisfy { $0.rim(glass) == Color(.glassBorder) })
      #expect(ShellPaneKind.window.fill(glass) == Color(.surfaceWindow))
      #expect(ShellPaneKind.window.role(glass) == .chrome)
    }
    let solid = Appearance(material: .solid)
    #expect(ShellPaneKind.sidebar.fill(solid) == Color(.surfaceSidebar))
    #expect(ShellPaneKind.inspector.fill(solid) == Color(.surfaceWindow))
    #expect(ShellPaneKind.allCases.allSatisfy { $0.rim(solid) == Color(.separator) })
  }

  @Test func inspectorTabsFollowTheDesignOrder() {
    #expect(InspectorTab.allCases.map(\.title) == ["Changes", "Plan", "Context", "Tasks", "Info"])
  }
}
