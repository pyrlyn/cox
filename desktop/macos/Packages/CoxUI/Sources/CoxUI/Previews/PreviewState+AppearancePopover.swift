// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the Appearance popover (T37.26): its values as mockup screens
// 28–29 show them, and the main screen with it open, redrawn from every change it reports as
// the app does. Separate from the other fixture files so this card adds its own.

import SwiftUI

extension PreviewState {
  /// The blur `docs/config.jsonschema` allows, in pt (T37.13).
  private static let blurRange = 0.0...60.0
  /// The mockup's reflection, 78 % of the range, and its Depth, "High".
  private static let reflection = 47.0
  private static let depth = 0.8

  /// The popover as mockup screen 28 (Frosted) or 29 (Glossy) shows it; Solid keeps the
  /// frosted values, which its disabled sliders show.
  static func appearance(_ material: GlassMaterial) -> AppearancePopover.State {
    let isGlossy = material == .glossy
    var state = AppearancePopover.State()
    state.material = material
    state.opacity =
      isGlossy ? MaterialToken.glossyWindowOpacity : MaterialToken.frostedWindowOpacity
    (state.blur, state.blurRange) = (isGlossy ? reflection : MaterialToken.frostedBlur, blurRange)
    (state.depth, state.tint) = (depth, true)
    state.transparencyText = isGlossy ? "70%" : "58%"
    (state.blurText, state.depthText) = (isGlossy ? "Strong" : "34 pt", "High")
    return state
  }

  /// Mockup screens 28–29: the main screen with the Appearance popover open.
  static func appearanceOpen(_ material: GlassMaterial) -> MainScreenState {
    var state = main
    state.toolbar.popover = .appearance
    state.appearance = appearance(material)
    return state
  }
}

/// The popover alone, with `material` picked.
struct AppearancePopoverSample: View {
  let material: GlassMaterial

  var body: some View {
    AppearancePopover(state: PreviewState.appearance(material)) { _ in }
  }
}

/// The main screen as the app runs it: the window draws from the popover's values, and each
/// change the popover reports lands in them, so the window redraws live.
struct LiveMainScreenSample: View {
  @State var state: MainScreenState
  @Environment(\.coxAppearance) private var base

  var body: some View {
    MainScreen(
      state: state,
      send: { intent in
        if case .appearance(let change) = intent { state.appearance.apply(change) }
      },
      transcript: { EmptyView() }, inspector: { _ in }
    )
    .environment(\.coxAppearance, state.appearance.applied(to: base))
  }
}

#Preview("live, frosted") {
  LiveMainScreenSample(state: PreviewState.appearanceOpen(.frosted))
    .frame(width: PreviewState.window.width, height: PreviewState.window.height)
    .padding(Space.xxl)
    .background(PreviewBackdrop())
}

extension PreviewState {
  /// The project config sets the material and Depth, so their controls are disabled.
  static var appearanceLocked: AppearancePopover.State {
    var state = appearance(.frosted)
    state.locked = [.material: "project", .depth: "project"]
    return state
  }
}
