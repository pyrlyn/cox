// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `SectionHeader` (DS§6.2 row `SectionHeader(title, trailing)`, the mockup's `.sect` and
// `.ih`): the uppercase label over a sidebar group or an inspector block, with an optional
// trailing view — a `CountBadge`, a button. Separate so every group is headed the same way;
// the container owns the padding around it.

import SwiftUI

/// `font.label` in `text.secondary`, uppercased, with `trailing` pushed to the far edge. Not the
/// mockup's tertiary: a label must stay readable on frosted glass (DS§8).
struct SectionHeader<Trailing: View>: View {
  let title: String
  let trailing: Trailing

  init(_ title: String, @ViewBuilder trailing: () -> Trailing) {
    self.title = title
    self.trailing = trailing()
  }

  var body: some View {
    HStack(spacing: Space.m) {
      Text(title)
        .textStyle(.label)
        .textCase(.uppercase)
        .foregroundStyle(Color(.textSecondary))
        .accessibilityAddTraits(.isHeader)
      Spacer(minLength: Space.m)
      trailing
    }
  }
}

extension SectionHeader where Trailing == EmptyView {
  init(_ title: String) {
    self.init(title) { EmptyView() }
  }
}

#Preview("title") {
  PreviewMatrix { SectionHeader(PreviewState.sectionTitle).frame(width: Size.sidebarWidth) }
}
#Preview("trailing") {
  PreviewMatrix {
    SectionHeader(PreviewState.sectionTitle) { CountBadge(PreviewState.count) }
      .frame(width: Size.sidebarWidth)
  }
}
