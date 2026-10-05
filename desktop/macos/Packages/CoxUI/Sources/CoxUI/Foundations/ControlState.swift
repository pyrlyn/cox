// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The interaction state a control face draws (DS§3.6, DS§6.1): rest, hovered, pressed or
// disabled, with the tint and lift each one gets. Separate so every button-like style resolves
// hover, press and `isEnabled` the same way, and a snapshot can draw any state by value.

import SwiftUI

/// What a control face shows; disabled wins over pressed, pressed over hovered.
enum ControlState: CaseIterable, Sendable {
  case rest, hovered, pressed, disabled

  init(isEnabled: Bool, isPressed: Bool, isHovered: Bool) {
    if !isEnabled {
      self = .disabled
    } else if isPressed {
      self = .pressed
    } else {
      self = isHovered ? .hovered : .rest
    }
  }

  /// The quiet fill laid over the face: hover takes the lighter step, press the stronger one.
  var tint: Color {
    switch self {
    case .hovered: Color(.fillPrimary)
    case .pressed: Color(.fillSecondary)
    case .rest, .disabled: .clear
    }
  }

  /// Pressing pushes a lifted face down to the surface; a disabled face does not lift.
  func elevation(_ level: ElevationToken) -> ElevationToken {
    switch self {
    case .rest, .hovered: level
    case .pressed, .disabled: .e0
    }
  }
}

/// Resolves a style's press, the pointer and `isEnabled` into a `ControlState` for `face`.
struct ControlStateReader<Face: View>: View {
  let isPressed: Bool
  @ViewBuilder let face: (ControlState) -> Face
  @Environment(\.isEnabled) private var isEnabled
  @State private var isHovered = false

  var body: some View {
    let state = ControlState(isEnabled: isEnabled, isPressed: isPressed, isHovered: isHovered)
    face(state)
      .onHover { isHovered = $0 }
      .animation(.cox(Motion.durationFast), value: state)
  }
}
