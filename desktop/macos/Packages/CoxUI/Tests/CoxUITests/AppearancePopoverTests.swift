// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Appearance popover's check (T37.26, DS§3.5, DS§6.4): the popover per picked material in
// every light/dark × Solid/Frosted cell; the main screen with it open per material, the window
// drawn from the popover's values; Reduce Transparency, where the window is Solid and the glass
// controls say why they are off; and how a control's change reaches the intent and the window.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct AppearancePopoverSnapshotTests {
  @Test(arguments: Variant.all) func appearancePopover(_ variant: Variant) throws {
    for material in MaterialPicker.order {
      try assertCoxSnapshot(
        AppearancePopoverSample(material: material), variant,
        named: "\(material.rawValue).\(variant.name)")
    }
  }

  /// The cell is Solid; the window takes the popover's material instead.
  @Test func mainScreenDrawsThePopoversMaterial() throws {
    for material in MaterialPicker.order {
      try assertCoxWindowSnapshot(
        LiveMainScreenSample(state: PreviewState.appearanceOpen(material)), Self.light,
        named: material.rawValue)
    }
  }

  @Test(arguments: [ColorScheme.light, .dark]) func reduceTransparencyDisablesTheGlassControls(
    _ scheme: ColorScheme
  ) throws {
    try assertCoxSnapshot(
      AppearancePopoverSample(material: .frosted), Variant(scheme: scheme, material: .frosted),
      reduceTransparency: true, named: scheme == .dark ? "dark" : "light")
  }

  @Test func reduceTransparencyWindowIsSolid() throws {
    try assertCoxWindowSnapshot(
      LiveMainScreenSample(state: PreviewState.appearanceOpen(.frosted)), Self.light,
      reduceTransparency: true, named: "frosted")
  }

  /// A frosted window under Reduce Transparency draws exactly as a Solid one.
  @Test func reduceTransparencyDrawsTheWindowAsSolid() throws {
    var frosted = PreviewState.main
    frosted.appearance = PreviewState.appearance(.frosted)
    var solid = frosted
    solid.appearance.material = .solid
    let forced = try SnapshotHost(window(frosted), Self.light, reduceTransparency: true).bitmap()
    let drawn = try SnapshotHost(window(solid), Self.light).bitmap()
    #expect(forced.tiffRepresentation == drawn.tiffRepresentation)
  }

  private static let light = Variant(scheme: .light, material: .solid)

  private func window(_ state: MainScreenState) -> some View {
    LiveMainScreenSample(state: state)
      .frame(width: PreviewState.window.width, height: PreviewState.window.height)
  }
}

@MainActor
@Suite struct AppearancePopoverTests {
  @Test func eachControlReportsItsConfigKey() {
    var sent: [AppearancePopover.Intent] = []
    let popover = AppearancePopover(state: PreviewState.appearance(.frosted)) { sent.append($0) }
    popover.transparency.wrappedValue = 0.7
    popover.bind(\.material, AppearancePopover.Intent.material).wrappedValue = .glossy
    popover.bind(\.blur, AppearancePopover.Intent.blur).wrappedValue = 12
    popover.bind(\.depth, AppearancePopover.Intent.depth).wrappedValue = 0.25
    popover.bind(\.tint, AppearancePopover.Intent.tint).wrappedValue = false
    #expect(sent == [.opacity(1 - 0.7), .material(.glossy), .blur(12), .depth(0.25), .tint(false)])
    #expect(popover.transparency.wrappedValue == 1 - MaterialToken.frostedWindowOpacity)
  }

  @Test func aChangeRedrawsTheWindowBeforeTheCoreAnswers() {
    var state = PreviewState.appearance(.frosted)
    state.apply(.material(.glossy))
    state.apply(.opacity(0.7))
    state.apply(.depth(0.25))
    state.apply(.blur(12))
    state.apply(.tint(false))
    let base = Appearance(material: .solid, textScale: 1.2)
    #expect(
      state.applied(to: base)
        == Appearance(material: .glossy, windowOpacity: 0.7, depth: 0.25, textScale: 1.2))
    #expect(state.blur == 12)
    #expect(!state.tint)
    #expect(state.transparencyText == PreviewState.appearance(.frosted).transparencyText)
  }
}
