// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `InlineCode` (DS§6.2 row `InlineCode, Hairline`, the mockup's `code`): a command, path or
// identifier set apart in a label or a notice. Separate so code outside the transcript looks
// like code inside it; the transcript's own prose draws its inline code in its text engine.

import SwiftUI

/// `font.mono.inline` on a quiet `fill.primary` face with a hairline rim.
struct InlineCode: View {
  let code: String

  /// The mockup's `padding: 1px 5px`: neither step is on the space scale.
  private static let verticalPadding: CGFloat = 1
  private static let horizontalPadding: CGFloat = 5

  init(_ code: String) {
    self.code = code
  }

  var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.badge, style: .continuous)
    Text(code)
      .textStyle(.monoInline)
      .lineLimit(1)
      .foregroundStyle(Color(.textPrimary))
      .padding(.horizontal, Self.horizontalPadding)
      .padding(.vertical, Self.verticalPadding)
      .background(Color(.fillPrimary), in: shape)
      .hairline(in: shape)
  }
}

#Preview { PreviewMatrix { InlineCode(PreviewState.inlineCode) } }
