// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `.textStyle(_:)` (DS§3.2, DS§6.1): font, line height, tracking and tabular digits from a
// `FontToken`, at the user's text size. Separate so no view spells a font and every text
// scales together.

import SwiftUI

extension View {
  /// Sets the token's font at the user's text size; `tabularDigits` for numbers that change
  /// while you watch (always on for `.metric`).
  func textStyle(_ token: FontToken, tabularDigits: Bool = false) -> some View {
    modifier(TextStyle(token: token, tabularDigits: tabularDigits || token == .metric))
  }

  /// An SF Symbol at the token's size, weight medium, rendered hierarchical (DS§3.7), so a
  /// glyph scales with the text beside it.
  func symbolStyle(_ token: FontToken = .body) -> some View {
    textStyle(token).fontWeight(.medium).symbolRenderingMode(.hierarchical)
  }
}

private struct TextStyle: ViewModifier {
  let token: FontToken
  let tabularDigits: Bool
  @EffectiveAppearance private var appearance

  func body(content: Content) -> some View {
    let size = token.size * appearance.textScale
    let font = Font.system(size: size, weight: token.weight, design: token.design)
    content
      .font(tabularDigits ? font.monospacedDigit() : font)
      // The line box, not `lineSpacing`, which only adds between lines: a single line is as
      // tall as the mockups' CSS line box, so rows and labels keep the designed rhythm.
      .lineHeight(.exact(points: size * token.lineHeight))
      .tracking(token.tracking * size)
  }
}
