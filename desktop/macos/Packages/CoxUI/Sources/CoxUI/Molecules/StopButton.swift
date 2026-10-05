// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `StopButton` (DS§6.3 row `StopButton`, the mockup's `.stop`): interrupts the running turn,
// with its shortcut beside it. Separate because it is the one capsule drawn inverted — the
// darkest thing in a light toolbar, the lightest in a dark one — so it is found at a glance.

import SwiftUI

/// A stop glyph, "Stop" and an inverted `KeyCap` on a `text.primary` capsule, lifted to e1 like the
/// capsules beside it. The shortcut it shows is the one it answers to.
struct StopButton: View {
  let action: () -> Void

  /// ⌘. — the macOS shortcut for cancelling an operation.
  private static let shortcut = KeyboardShortcut(".", modifiers: .command)

  init(action: @escaping () -> Void) {
    self.action = action
  }

  var body: some View {
    Button(action: action) {
      HStack(spacing: Space.s) {
        Image(systemName: "stop.fill").symbolStyle(.caption)
        Text("Stop")
        KeyCap("⌘.", inverted: true)
      }
    }
    .buttonStyle(StopStyle())
    .keyboardShortcut(Self.shortcut)
    .help("Stop the turn (⌘.)")
  }
}

/// The inverted capsule face in each `ControlState`: hover and press lay the usual quiet fills
/// over it, press sinks it to e0.
private struct StopStyle: ButtonStyle {
  func makeBody(configuration: Configuration) -> some View {
    ControlStateReader(isPressed: configuration.isPressed) { state in
      let shape = RoundedRectangle(cornerRadius: Radius.capsule, style: .continuous)
      configuration.label
        .textStyle(.stop)
        .foregroundStyle(Color(.surfaceWindow))
        .padding(.horizontal, Space.l)
        .frame(height: Size.capsuleHeight)
        .background { shape.fill(state.tint) }
        .background { shape.fill(Color(.textPrimary)) }
        .elevation(state.elevation(.e1), cornerRadius: Radius.capsule)
        .contentShape(shape)
    }
  }
}

#Preview { PreviewMatrix { StopButton {} } }
