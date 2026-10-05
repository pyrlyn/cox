// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `SettingLabel` (DS§6.3 rows `LabeledToggle` and `SettingRow`, the Settings `.gr .l` with its
// `small`): a setting's name over the line saying what it does. Separate so a switch and any
// other setting control name their setting the same way. Also marks where the Settings search
// matched a title (T37.45.1), so the sidebar's pages and the fields' labels mark it alike.

import SwiftUI

/// The title in `font.body` over an optional `text.secondary` detail in `font.footnote`.
struct SettingLabel: View {
  @Environment(\.settingsFilter) private var filter
  let title: String
  /// What the setting does, `for new sessions`, or `nil`.
  let detail: String?
  /// The row names a thing — a server, a check — rather than a setting: its title semibold, the
  /// mockup's `<b>`.
  let namesItem: Bool

  init(_ title: String, detail: String? = nil, namesItem: Bool = false) {
    self.title = title
    self.detail = detail
    self.namesItem = namesItem
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      Text(AttributedString(title, marking: filter, on: Color(.accentSoft)))
        .textStyle(.body)
        .fontWeight(namesItem ? .semibold : nil)
        .foregroundStyle(Color(.textPrimary))
      if let detail {
        Text(detail)
          .textStyle(.footnote)
          .foregroundStyle(Color(.textSecondary))
      }
    }
  }
}

extension EnvironmentValues {
  /// The Settings search; each `SettingLabel` marks where its title holds it.
  @Entry var settingsFilter = ""
}

extension AttributedString {
  /// `text` with each stretch that holds `query` — ignoring case and diacritics, as CoxModel's
  /// `localizedStandardContains` filters — in bold, on `mark` when one is given.
  init(_ text: String, marking query: String, on mark: Color?) {
    self.init(text)
    let query = query.trimmingCharacters(in: .whitespaces)
    guard !query.isEmpty else { return }
    let options: String.CompareOptions = [.caseInsensitive, .diacriticInsensitive]
    var rest = text.startIndex..<text.endIndex
    while let found = text.range(of: query, options: options, range: rest, locale: .current) {
      if let range = Range(found, in: self) {
        self[range].inlinePresentationIntent = .stronglyEmphasized
        self[range].backgroundColor = mark
      }
      rest = found.upperBound..<text.endIndex
    }
  }
}
