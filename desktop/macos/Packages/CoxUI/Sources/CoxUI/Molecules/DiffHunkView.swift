// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `DiffHunkView` (DS§6.3 row `DiffHunkView`, the mockup's `.diff` with its `.hh`): one hunk of a
// diff — the `@@` header and its lines. Separate from `DiffLineView` so the header and the
// shared gutter width belong to the hunk, and a card or the review pane stacks hunks as units.
// In Review the header carries "Revert hunk" (T51.21), shown while the hunk is hovered.

import SwiftUI

/// The header in `text.secondary` on `fill.primary`, then the lines on `surface.code`, which
/// holds the readable floor under code (DS§3.5).
struct DiffHunkView: View {
  /// The core's hunk header, `@@ -41,12 +41,26 @@ impl Backoff`.
  let header: String
  let lines: [DiffLineView.Line]
  /// Review's click on a line's number, by its index in `lines` (T37.28.4).
  let comment: (@MainActor (Int) -> Void)?
  /// Review's "Revert hunk" (T51.21); `nil` hides it, as on an edit card.
  let revert: (@MainActor () -> Void)?
  @State private var isHovered: Bool

  /// `isHovered` is where hover starts, so a snapshot can show the revealed button.
  init(
    header: String, lines: [DiffLineView.Line], comment: (@MainActor (Int) -> Void)? = nil,
    revert: (@MainActor () -> Void)? = nil, isHovered: Bool = false
  ) {
    self.header = header
    self.lines = lines
    self.comment = comment
    self.revert = revert
    self._isHovered = State(initialValue: isHovered)
  }

  var body: some View {
    let widest = lines.map(\.number).max { $0.count < $1.count }
    VStack(alignment: .leading, spacing: 0) {
      HStack(spacing: Space.m) {
        Text(header)
          .textStyle(.monoCode)
          .foregroundStyle(Color(.textSecondary))
          .lineLimit(1)
          .frame(maxWidth: .infinity, alignment: .leading)
        if let revert {
          // Always laid out, so revealing it never moves the header.
          Button(action: revert) {
            Label("Revert hunk", systemImage: "arrow.uturn.backward")
              .textStyle(.caption)
          }
          .buttonStyle(.plain)
          .foregroundStyle(Color(.textSecondary))
          .help("Revert hunk (⌥-click skips the confirmation)")
          .opacity(isHovered ? 1 : 0)
          .accessibilityHidden(true)
        }
      }
      .padding(.horizontal, Space.ml)
      .padding(.vertical, Space.xxs)
      .background(Color(.fillPrimary))
      .accessibilityElement(children: .combine)
      .accessibilityActions {
        if let revert { Button("Revert hunk", action: revert) }
      }
      ForEach(lines.indices, id: \.self) { index in
        DiffLineView(
          lines[index], widestNumber: widest, comment: comment.map { tap in { tap(index) } })
      }
    }
    .padding(.bottom, Space.s)
    .background(Color(.surfaceCode))
    .onHover { isHovered = $0 }
  }
}

#Preview { PreviewMatrix { DiffHunkSample() } }
#Preview("revert, hovered") { PreviewMatrix { DiffHunkSample(revert: true, isHovered: true) } }
