// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The token meter's generated `MeterText` ⇄ `CoxClient.MeterText`, field for field (T37.25, A98).
// Separate from `Convert.swift` so the meter's text can grow without that file doing so.

import CoxClient
import CoxFFIBindings

extension CoxClient.MeterText {
  init(_ value: CoxFFIBindings.MeterText) {
    self.init()
    (sent, received, rate, spoken) = (value.sent, value.received, value.rate, value.spoken)
    (heading, phase, rateUnit, rateDetail) = (
      value.heading, value.phase, value.rateUnit, value.rateDetail
    )
    rows = value.rows.map {
      .init(label: $0.label, turn: $0.turn, session: $0.session, detail: $0.detail)
    }
    (context, footnote) = (value.context, value.footnote)
    (contextShare, contextFree, cacheHit) = (value.contextShare, value.contextFree, value.cacheHit)
    cacheHitSession = value.cacheHitSession
    contextParts = value.contextParts.map {
      .init(kind: $0.kind, label: $0.label, tokens: $0.tokens, share: $0.share)
    }
    (contextPercent, contextFill, cost) = (value.contextPercent, value.contextFill, value.cost)
  }
}
