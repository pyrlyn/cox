// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `CountBadge` (DS§6.2 row `CountBadge`, the mockup's `.sect .cnt`): how many things wait for
// you, beside a section header. Separate from `Badge` because it is a solid, lifted pill that
// asks to be looked at, not a quiet tag.

import SwiftUI

/// `text.onAccent` tabular digits on a `status.warning` pill at e1, at least as wide as it is
/// tall.
struct CountBadge: View {
  /// The count as Rust formats it ("3", "99+").
  let count: String

  init(_ count: String) {
    self.count = count
  }

  var body: some View {
    Text(count)
      .textStyle(.micro, tabularDigits: true)
      .foregroundStyle(Color(.textOnAccent))
      .padding(.horizontal, Space.s)
      .frame(minWidth: Size.countBadge, minHeight: Size.countBadge)
      .background(Color(.statusWarning), in: Capsule())
      .elevation(.e1, cornerRadius: Size.countBadge / 2)
  }
}

#Preview { PreviewMatrix { CountBadge(PreviewState.count) } }
