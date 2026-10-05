// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `NoticeRow` (DS§6.3 row `NoticeRow, TurnDivider, TurnMeta`, the mockup's `.notice`): a line
// the app, not the model, adds to the transcript — Bypass is on, the session is driven over
// ACP, a request failed and is retried. Separate so every such line has one shape and says how
// serious it is by one colour.

import SwiftUI

/// A symbol in the kind's colour, then the text in `font.caption`. Warnings and errors keep
/// their text in `text.primary`: the colour is on the symbol, so the words stay readable on
/// frosted glass (DS§8).
struct NoticeRow: View {
  enum Kind: CaseIterable, Sendable {
    case info, warning, error
  }

  /// Plain words, or runs a caller marked, as `AcpBanner` bolds the agent's name.
  let text: AttributedString
  let kind: Kind
  /// A DS§3.7 symbol for what the notice is about, or the kind's own.
  let symbol: String

  init(_ text: String, kind: Kind = .info, symbol: String? = nil) {
    self.init(AttributedString(text), kind: kind, symbol: symbol)
  }

  init(_ text: AttributedString, kind: Kind = .info, symbol: String? = nil) {
    self.text = text
    self.kind = kind
    self.symbol = symbol ?? kind.symbol
  }

  var body: some View {
    HStack(alignment: .firstTextBaseline, spacing: Space.m) {
      Image(systemName: symbol)
        .symbolStyle(.body)
        .foregroundStyle(kind.tint)
        .accessibilityHidden(true)
      Text(text)
        .textStyle(.caption)
        .foregroundStyle(kind.foreground)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
    .accessibilityElement(children: .combine)
  }
}

extension NoticeRow.Kind {
  var symbol: String {
    switch self {
    case .info: "info.circle"
    case .warning: "exclamationmark.triangle"
    case .error: "xmark.octagon"
    }
  }

  var tint: Color {
    switch self {
    case .info: Color(.textSecondary)
    case .warning: Color(.statusWarning)
    case .error: Color(.statusDanger)
    }
  }

  var foreground: Color {
    switch self {
    case .info: Color(.textSecondary)
    case .warning, .error: Color(.textPrimary)
    }
  }
}

#Preview("info") { PreviewMatrix { NoticeRowSample(kind: .info) } }
#Preview("warning") { PreviewMatrix { NoticeRowSample(kind: .warning) } }
#Preview("error") { PreviewMatrix { NoticeRowSample(kind: .error) } }
