// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `DiffStat` (DS§6.2 row `DiffStat(added, removed)`, the mockup's `.plus` and `.minus`): the
// lines a change adds and removes, "+42 −7". Separate so a tool row, a turn summary and the
// changes tab count lines the same way.

import SwiftUI

/// Tabular `+added` in `status.success` and `−removed` in `status.danger`; an add-only change
/// (a new file) shows no "−0".
struct DiffStat: View {
  let added: Int
  let removed: Int

  var body: some View {
    HStack(spacing: Space.xs) {
      Text(verbatim: "+\(added)").foregroundStyle(Color(.statusSuccess))
      if showsRemoved {
        Text(verbatim: "−\(removed)").foregroundStyle(Color(.statusDanger))
      }
    }
    .textStyle(.footnote, tabularDigits: true)
    .fontWeight(.semibold)
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(label)
  }

  var showsRemoved: Bool { removed > 0 }

  var label: String {
    showsRemoved ? "\(added) lines added, \(removed) removed" : "\(added) lines added"
  }
}

#Preview { PreviewMatrix { DiffStat(added: PreviewState.added, removed: PreviewState.removed) } }
