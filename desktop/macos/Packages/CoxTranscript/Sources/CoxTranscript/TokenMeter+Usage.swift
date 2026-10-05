// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The token meter and popover's values from the core's `UsageView` (T37.25, DS§7): the figures
// `cox_app::MeterText` formatted, copied field by field. Here, beside `SessionComposer`, because
// this package is where CoxUI's values and CoxClient's types meet; nothing here computes a
// figure. The context split (A98) comes from CoxModel's `ContextSplit`, the one mapping the popover
// and the inspector's Context tab share; `TokenPopover.Part.init(_:)` only turns its part into CoxUI's.

import CoxClient
import CoxModel
import CoxUI

extension TokenMeter.State {
  /// The meter for `usage`; `isRunning` makes the dot glow.
  init(_ usage: UsageView, isRunning: Bool) {
    self.init()
    let text = usage.text
    (sent, received, rate, spoken) = (text.sent, text.received, text.rate, text.spoken)
    isStreaming = isRunning
    sparkline = usage.turn?.sparkline ?? []
  }
}

extension TokenPopover.State {
  /// The popover for `usage`; `isRunning` puts the phase in `accent`.
  init(_ usage: UsageView, isRunning: Bool) {
    self.init()
    let text = usage.text
    (heading, phase, isStreaming) = (text.heading, text.phase, isRunning)
    (rate, rateUnit, rateDetail) = (text.rate, text.rateUnit, text.rateDetail)
    sparkline = usage.turn?.sparkline ?? []
    rows = text.rows.map {
      TokenPopover.Row(label: $0.label, turn: $0.turn, session: $0.session, isDetail: $0.detail)
    }
    (context, contextShare, footnote) = (text.context, text.contextShare, text.footnote)
    parts = ContextSplit(text).parts.compactMap(TokenPopover.Part.init)
  }
}

extension TokenPopover.Part {
  /// The bar segment and legend (`System 3.5k`) of `part`; `ContextSplit` already dropped a kind
  /// the bar has no colour role for, so `nil` only if the two kind lists ever drift apart.
  init?(_ part: ContextSplit.Part) {
    guard let kind = StackedBar.Kind(rawValue: part.kind.rawValue) else { return nil }
    self.init(kind: kind, fraction: part.fraction, legend: "\(part.label) \(part.tokens)")
  }
}
