// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ModelPopover` (DS§6.4 row `Composer`, its model chip's popover, the mockup's
// `.popover`; DT§5.1; T37.22.6, T60.7): the models of each provider under its header, the one
// the session runs on marked, a click switching to another; a provider with no key is greyed and
// its header offers "Add key". Separate so the composer only opens it and the screen hangs it over
// the chip; the rows, their ids and efforts all come from the core's catalog through CoxModel's
// `ModelMenu`.

import SwiftUI

/// A header per provider section over its rows on readable popover glass at e4, as
/// `CompletionList` draws them: the model's id, its efforts at the trailing edge, the running one
/// on `accent`. A section that cannot pick shows its rows in `text.tertiary` and, when a key
/// would fix it, "Add key" at the header's trailing edge.
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
    /// The `[providers.<name>]` section; "Add key" reports it.
    public var provider: String
    /// The rows pick; off, they are greyed.
    public var isEnabled: Bool
    /// The header offers "Add key": the provider has no key.
    public var offersKey: Bool
    /// Why the rows do not pick when no key would fix it, as a caption under the header.
    public var note: String?

    public init(
      title: String, rows: [CompletionList.Row], selected: String? = nil, provider: String = "",
      isEnabled: Bool = true, offersKey: Bool = false, note: String? = nil
    ) {
      (self.title, self.rows, self.selected) = (title, rows, selected)
      (self.provider, self.isEnabled, self.offersKey, self.note) = (
        provider, isEnabled, offersKey, note
      )
    }
  }

  let state: State
  let pick: (String) -> Void
  let addKey: (String) -> Void

  public init(
    state: State, pick: @escaping (String) -> Void, addKey: @escaping (String) -> Void = { _ in }
  ) {
    self.state = state
    self.pick = pick
    self.addKey = addKey
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
        SectionHeader(section.title) {
          if section.offersKey {
            Button("Add key") { addKey(section.provider) }
              .buttonStyle(CoxButtonStyle(.plain, size: .small))
              .help("Open Settings to add this provider's key")
          }
        }
        .padding(.horizontal, Space.ml)
        .padding(.top, Space.s)
        .padding(.bottom, Space.xs)
        if let note = section.note {
          Text(note)
            .textStyle(.footnote)
            .foregroundStyle(Color(.textSecondary))
            .padding(.horizontal, Space.ml)
            .padding(.bottom, Space.xs)
        }
        ForEach(section.rows) { row in
          CompletionRow(
            row: row, isSelected: row.id == section.selected, isEnabled: section.isEnabled
          ) { pick(row.id) }
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

/// Where the composer's model chip sits, so the screen hangs the popover over it.
struct ModelChipAnchor: PreferenceKey {
  static var defaultValue: Anchor<CGRect>? { nil }

  static func reduce(value: inout Anchor<CGRect>?, nextValue: () -> Anchor<CGRect>?) {
    value = value ?? nextValue()
  }
}

#Preview("tiers") { PreviewMatrix { ModelPopover(state: PreviewState.models) { _ in } } }
#Preview("two providers") {
  PreviewMatrix { ModelPopover(state: PreviewState.modelsOfTwoProviders) { _ in } }
}
#Preview("empty") { PreviewMatrix { ModelPopover(state: ModelPopover.State()) { _ in } } }
