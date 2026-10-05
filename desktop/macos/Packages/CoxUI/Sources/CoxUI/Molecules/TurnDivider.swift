// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `TurnDivider` (DS§6.3 row `NoticeRow, TurnDivider, TurnMeta`, the mockup's `.divider`): the
// rule between turns, optionally naming what happened there — a compaction, a resumed session,
// a new day. Separate so every break in the transcript is drawn by the same two hairlines.

import SwiftUI

/// A `Hairline` across the column, or two with the label in `font.caption` between them.
struct TurnDivider: View {
  /// What the break is, `Compacted · 48.2k → 12.1k`, or `nil` for a bare rule.
  let label: String?

  init(_ label: String? = nil) {
    self.label = label
  }

  var body: some View {
    HStack(spacing: Space.l) {
      Hairline()
      if let label {
        Text(label)
          .textStyle(.caption)
          .foregroundStyle(Color(.textSecondary))
          .lineLimit(1)
          .fixedSize()
        Hairline()
      }
    }
  }
}

#Preview("label") { PreviewMatrix { TurnDividerSample(label: PreviewState.dividerLabel) } }
#Preview("bare") { PreviewMatrix { TurnDividerSample(label: nil) } }
