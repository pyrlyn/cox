// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `SettingsGroupBox` (DS§6.4 row `SettingsGroupBox`, the mockup's Settings `.gtitle` over a
// `.group`): the settings of one config table as a card — a header naming the table, then the
// rows with a hairline between each two. Separate so every Settings page draws its boxes the
// same way, whatever rows they hold.

import SwiftUI

/// An optional title in `font.title.group` over `content`'s rows on a `fill.primary` card of
/// `Radius.xl`, each row after the first under a hairline.
struct SettingsGroupBox<Content: View>: View {
  /// The header, or `nil` for a box its rows explain, as the first-run window's.
  let title: String?
  let content: Content

  init(_ title: String?, @ViewBuilder content: () -> Content) {
    self.title = title
    self.content = content()
  }

  var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.xl, style: .continuous)
    VStack(alignment: .leading, spacing: Space.s) {
      // Level with the rows' text inside the card.
      if let title {
        // The mockup's `.gtitle`: sentence case in `text.secondary`, not a sidebar `SectionHeader`.
        Text(title)
          .textStyle(.titleGroup)
          .foregroundStyle(Color(.textSecondary))
          .accessibilityAddTraits(.isHeader)
          .padding(.horizontal, Space.xs)
      }
      Group(subviews: content) { rows in
        VStack(spacing: 0) {
          ForEach(rows) { row in row.hairline(row.id == rows.first?.id ? [] : .top) }
        }
      }
      .background(Color(.fillPrimary), in: shape)
      .clipShape(shape)
    }
  }
}

#Preview("rows") {
  PreviewMatrix {
    SettingsGroupBox(PreviewState.boxTitle) {
      SettingRowSample.control
      SettingRowSample.readOnly
    }
  }
}
