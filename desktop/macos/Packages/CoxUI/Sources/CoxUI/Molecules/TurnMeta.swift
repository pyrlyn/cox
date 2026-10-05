// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `TurnMeta` (DS§6.3 row `NoticeRow, TurnDivider, TurnMeta`, the mockup's `.meta`): the quiet
// line under a finished turn — model, tokens, cache share, cost, how long it took and why it
// stopped. Separate so every turn reports its figures in the same order and the same place.

import SwiftUI

/// The facts the core formatted, in a row of `font.footnote` with tabular digits.
struct TurnMeta: View {
  struct Facts: Equatable, Sendable {
    /// `Sonnet 5`, or `nil` when the turn used the session's model.
    var model: String?
    /// `in 48.2k · out 3.1k`.
    var tokens: String
    /// `cache 91%`, or `nil` when nothing was cached.
    var cache: String?
    var cost: String
    var duration: String
    /// `end turn`, `max tokens`, or `nil`.
    var stopReason: String?

    /// The facts shown, in the mockup's order.
    var parts: [String] {
      [model, tokens, cache, cost, duration, stopReason].compactMap(\.self)
    }
  }

  let facts: Facts

  init(_ facts: Facts) {
    self.facts = facts
  }

  var body: some View {
    HStack(spacing: Space.l) {
      ForEach(Array(facts.parts.enumerated()), id: \.offset) { Text($0.element) }
    }
    .textStyle(.footnote, tabularDigits: true)
    // `text.secondary`, not the mockup's tertiary: figures must stay readable on frosted
    // glass (DS§8).
    .foregroundStyle(Color(.textSecondary))
    .lineLimit(1)
    .accessibilityElement(children: .combine)
  }
}

#Preview("full") { PreviewMatrix { TurnMeta(PreviewState.turnMeta) } }
#Preview("short") { PreviewMatrix { TurnMeta(PreviewState.turnMetaShort) } }
