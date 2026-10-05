// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ModelPopover` (DS§6.4 row `SessionToolbar`, its model capsule's popover, the mockup's
// `.popover`; DT§5.1; T37.22.6): each tier's models under the tier's name, the one the session
// runs on marked, a click switching to another. Separate so the toolbar only opens it and the
// screen hangs it under the capsule; the rows, their ids and efforts all come from the core's
// catalog through CoxModel's `ModelMenu`.

import SwiftUI

/// A header per tier over its rows on readable popover glass at e4, as `CompletionList` draws
/// them: the model's id, its efforts at the trailing edge, the running one on `accent.soft`.
public struct ModelPopover: View {
  public struct State: Equatable, Sendable {
    public var sections: [Section]

    public init(sections: [Section] = []) { self.sections = sections }
  }

  /// One tier: its title and models; a row's `id` is what a click reports.
  public struct Section: Equatable, Sendable, Identifiable {
    public var id: String { title }
    public var title: String
    public var rows: [CompletionList.Row]
    /// The row the session runs on, when this tier lists it.
    public var selected: String?

    public init(title: String, rows: [CompletionList.Row], selected: String? = nil) {
      (self.title, self.rows, self.selected) = (title, rows, selected)
    }
  }

  let state: State
  let pick: (String) -> Void

  public init(state: State, pick: @escaping (String) -> Void) {
    self.state = state
    self.pick = pick
  }

  public var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.popover, style: .continuous)
    VStack(alignment: .leading, spacing: 0) {
      if state.sections.isEmpty {
        Text("The config lists no models.")
          .textStyle(.footnote)
          .foregroundStyle(Color(.textSecondary))
          .padding(Space.ml)
      }
      ForEach(state.sections) { section in
        SectionHeader(section.title)
          .padding(.horizontal, Space.ml)
          .padding(.top, Space.s)
          .padding(.bottom, Space.xs)
        ForEach(section.rows) { row in
          CompletionRow(row: row, isSelected: row.id == section.selected) { pick(row.id) }
        }
      }
    }
    .padding(Space.s)
    .frame(width: Size.popoverWidth, alignment: .leading)
    .glassPane(shape, surface: Color(.surfacePopover), role: .readable)
    .hairline(in: shape)
    .elevation(.e4, cornerRadius: Radius.popover)
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Model")
  }
}

/// Where the toolbar's model capsule sits, so the screen hangs the popover under it.
struct ModelCapsuleAnchor: PreferenceKey {
  static var defaultValue: Anchor<CGRect>? { nil }

  static func reduce(value: inout Anchor<CGRect>?, nextValue: () -> Anchor<CGRect>?) {
    value = value ?? nextValue()
  }
}

#Preview("tiers") { PreviewMatrix { ModelPopover(state: PreviewState.models) { _ in } } }
#Preview("empty") { PreviewMatrix { ModelPopover(state: ModelPopover.State()) { _ in } } }
