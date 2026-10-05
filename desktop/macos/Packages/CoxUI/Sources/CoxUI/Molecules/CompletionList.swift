// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `CompletionList` (DS§6.3 row `CompletionList`, the mockup's `.pop`; DT§5.3; mockup screens
// 5–6): the rows the composer offers for the token being typed — files for `@`, commands for
// `/` — ranked by the core, with one row picked. Separate so the composer only places it; the
// rows, their order and their text all come from `cox-app`'s completer.

import SwiftUI

/// A header over the rows on readable popover glass at e4; the selected row is the mockup's
/// `.it.on`, `text.onAccent` on an `accent` face. A click picks a row.
public struct CompletionList: View {
  public struct State: Equatable, Sendable {
    /// What the rows are, `Files` or `Commands`.
    public var title: String
    public var rows: [Row]
    /// The row ⏎ or ⇥ would pick.
    public var selection: Int

    public init(title: String, rows: [Row], selection: Int = 0) {
      self.title = title
      self.rows = rows
      self.selection = selection
    }
  }

  /// One row as the core formatted it: what it inserts and its second line.
  public struct Row: Equatable, Sendable, Identifiable {
    public var id: String
    public var title: String
    public var detail: String

    public init(id: String, title: String, detail: String) {
      self.id = id
      self.title = title
      self.detail = detail
    }
  }

  let state: State
  let pick: (Int) -> Void

  public init(state: State, pick: @escaping (Int) -> Void) {
    self.state = state
    self.pick = pick
  }

  /// The mockup's `.pop`: a path and its detail need the room.
  static let width = Size.completionWidth

  public var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.xxl, style: .continuous)
    VStack(alignment: .leading, spacing: 0) {
      SectionHeader(state.title)
        .padding(.horizontal, Space.ml)
        .padding(.top, Space.s)
        .padding(.bottom, Space.xs)
      ForEach(Array(state.rows.enumerated()), id: \.element.id) { index, row in
        CompletionRow(row: row, isSelected: index == state.selection) { pick(index) }
      }
    }
    .padding(Space.s)
    .frame(width: Self.width, alignment: .leading)
    .glassPane(shape, surface: Color(.surfacePopover), role: .readable)
    .hairline(in: shape)
    .elevation(.e4, cornerRadius: Radius.xxl)
    .accessibilityElement(children: .contain)
    .accessibilityLabel(state.title)
  }
}

/// The row's title in `font.body`, its detail in `font.footnote` at the trailing edge; the model
/// popover's rows too.
struct CompletionRow: View {
  let row: CompletionList.Row
  let isSelected: Bool
  let action: () -> Void

  var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.m, style: .continuous)
    Button(action: action) {
      HStack(spacing: Space.ml) {
        Text(row.title)
          .textStyle(.body)
          .foregroundStyle(isSelected ? Color(.textOnAccent) : Color(.textPrimary))
          .lineLimit(1)
          .truncationMode(.middle)
        Spacer(minLength: Space.m)
        Text(row.detail)
          .textStyle(.footnote)
          .foregroundStyle(isSelected ? Color(.textOnAccent) : Color(.textSecondary))
          .lineLimit(1)
          .truncationMode(.middle)
      }
      .padding(.horizontal, Space.ml)
      .padding(.vertical, Space.s)
      .background { if isSelected { shape.fill(Color(.accent)) } }
      .contentShape(shape)
    }
    .buttonStyle(.plain)
    .accessibilityAddTraits(isSelected ? .isSelected : [])
  }
}

#Preview("files") {
  PreviewMatrix { CompletionList(state: PreviewState.fileCompletion) { _ in } }
}
#Preview("commands") {
  PreviewMatrix { CompletionList(state: PreviewState.commandCompletion) { _ in } }
}
