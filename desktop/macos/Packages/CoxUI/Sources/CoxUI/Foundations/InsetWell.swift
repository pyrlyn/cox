// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `.insetWell()` (DS§6.1): the pressed-in look of the terminal tail and text fields, the
// mockup's `.tail` and `.filter`. Separate because it is the one inner shadow in the app; like
// elevation it fades with Depth, so Flat gives a plain filled field.

import SwiftUI

extension View {
  /// Backs the view with a sunken `surface` of `cornerRadius`, a hairline rim and an inner shadow.
  func insetWell(
    _ surface: Color = Color(.surfaceTerminal), cornerRadius: CGFloat = Radius.m
  ) -> some View {
    modifier(InsetWell(surface: surface, cornerRadius: cornerRadius))
  }
}

private struct InsetWell: ViewModifier {
  let surface: Color
  let cornerRadius: CGFloat
  @EffectiveAppearance private var appearance

  /// The mockup's `inset 0 2px 8px rgba(0,0,0,.35)`, tinted like every other shadow.
  private static let shadowOpacity = 0.35
  private static let shadowBlur: CGFloat = 8
  private static let shadowY: CGFloat = 2

  func body(content: Content) -> some View {
    let shape = RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
    content
      .background {
        shape.fill(
          surface.shadow(
            .inner(
              color: Color(.shadowTint).opacity(Self.shadowOpacity * appearance.depth),
              radius: Self.shadowBlur / 2, y: Self.shadowY * appearance.depth)))
      }
      .clipShape(shape)
      .hairline(in: shape)
  }
}
