// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `InspectorRow` (DS§6.3 row `ChangedFileRow`, `CheckpointRow`, the mockup's `.fr`): the line
// the inspector lists things on — a glyph, what the row names, a trailing figure and the row's
// actions. Separate so a changed file and a checkpoint share one layout, selection and action
// strip instead of each drawing its own.

import SwiftUI

/// Something a row lets you do to its item, `Revert` or `Rewind`.
struct RowAction: Identifiable, Sendable {
  let title: String
  /// The DS§3.7 symbol of the action's icon button.
  let symbol: String
  let perform: @MainActor () -> Void

  var id: String { title }
}

/// The glyph, then `content` (the name and its trailing figure), then the actions as icon
/// buttons while the row is hovered or selected. The selected row sits on `accent.soft`, lifted
/// to e1 like a `SessionRow`; VoiceOver reads the row as one element with the actions attached.
struct InspectorRow<Glyph: View, Content: View>: View {
  let glyph: Glyph
  let isSelected: Bool
  let actions: [RowAction]
  let content: Content
  @State private var isHovered = false

  /// A row led by `glyph`, as a Settings page's `IconTile`.
  init(
    glyph: Glyph, isSelected: Bool, actions: [RowAction], @ViewBuilder content: () -> Content
  ) {
    self.glyph = glyph
    self.isSelected = isSelected
    self.actions = actions
    self.content = content()
  }

  var body: some View {
    HStack(spacing: Space.m) {
      glyph
      content
      if isHovered || isSelected {
        ForEach(actions) { RowActionButton(action: $0) }
      }
    }
    .textStyle(.compact)
    .foregroundStyle(Color(.textPrimary))
    .lineLimit(1)
    .padding(.horizontal, Space.m)
    .padding(.vertical, Space.s)
    .onHover { isHovered = $0 }
    .rowSelection(isSelected)
    .accessibilityActions {
      ForEach(actions) { action in Button(action.title, action: action.perform) }
    }
  }
}

extension InspectorRow where Glyph == SymbolGlyph {
  /// A row led by the DS§3.7 `symbol` at body size.
  init(
    symbol: String, isSelected: Bool, actions: [RowAction], @ViewBuilder content: () -> Content
  ) {
    self.init(
      glyph: SymbolGlyph(symbol: symbol), isSelected: isSelected, actions: actions,
      content: content)
  }
}

extension View {
  /// A list row's selection, shared by `InspectorRow` and `SessionRow`: selected, it sits on
  /// `fill` (`accent.soft`, or the session row's `accent.selected`, A115) lifted to e1 in a
  /// `radius.l` shape; VoiceOver reads the row as one element marked selected.
  func rowSelection(_ isSelected: Bool, fill: ColorResource = .accentSoft) -> some View {
    modifier(RowSelection(isSelected: isSelected, fill: fill))
  }
}

private struct RowSelection: ViewModifier {
  let isSelected: Bool
  let fill: ColorResource

  func body(content: Content) -> some View {
    let shape = RoundedRectangle(cornerRadius: Radius.l, style: .continuous)
    content
      .background { if isSelected { shape.fill(Color(fill)) } }
      .elevation(isSelected ? .e1 : .e0, cornerRadius: Radius.l)
      .contentShape(shape)
      .accessibilityElement(children: .combine)
      .accessibilityAddTraits(isSelected ? .isSelected : [])
  }
}

/// A row's plain symbol, in the row's text colour, centred in a column one body em wide (the
/// mockup's 13 pt `.fr` icon box), so every label starts at the same x whatever the symbol's
/// width: `doc.text` is narrower than `pencil`, `terminal` than `person.2`.
struct SymbolGlyph: View {
  let symbol: String
  @Environment(\.coxAppearance) private var appearance

  var body: some View {
    Image(systemName: symbol).symbolStyle(.body)
      .frame(width: FontToken.body.size * appearance.textScale)
  }
}

/// An action's glyph as a bare button, named by its tooltip (DS§8); a line high, so showing
/// the strip does not change the row's height.
private struct RowActionButton: View {
  let action: RowAction

  var body: some View {
    Button(action: action.perform) {
      Image(systemName: action.symbol).symbolStyle(.footnote)
    }
    .buttonStyle(.plain)
    .foregroundStyle(Color(.textSecondary))
    .help(action.title)
    .accessibilityHidden(true)
  }
}
