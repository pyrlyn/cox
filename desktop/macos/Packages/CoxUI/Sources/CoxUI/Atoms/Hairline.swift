// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `Hairline` (DS§6.2 row `InlineCode, Hairline`, the mockup's separators: `.divider:before`,
// `.popover .sep`): a free-standing 0.5 pt rule between rows, menu sections or toolbar groups.
// Separate from the `.hairline(_:)` modifier (Foundations/HairlineModifier.swift), which draws
// the same line on a view's edge; this one takes its own place in a stack.

import SwiftUI

/// A `Size.hairline` rule in `separator`, as long as its container along `orientation`.
struct Hairline: View {
  enum Orientation: CaseIterable, Sendable {
    case horizontal, vertical
  }

  let orientation: Orientation

  init(_ orientation: Orientation = .horizontal) {
    self.orientation = orientation
  }

  var body: some View {
    let across = orientation == .horizontal
    // Drawn by the modifier so a rule and an edge keep one width and one colour.
    Color.clear
      .frame(width: across ? nil : Size.hairline, height: across ? Size.hairline : nil)
      .hairline(across ? .top : .leading)
      .accessibilityHidden(true)
  }
}

#Preview("horizontal") {
  PreviewMatrix { Hairline(.horizontal).frame(width: Size.popoverWidth) }
}
#Preview("vertical") {
  PreviewMatrix { Hairline(.vertical).frame(height: Size.capsuleHeight) }
}
