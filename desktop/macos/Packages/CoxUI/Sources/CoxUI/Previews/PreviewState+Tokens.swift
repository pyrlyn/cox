// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the token meter and popover (T37.25): mockup screen 30's figures,
// as `cox_app::MeterText` formats them, streaming and after the turn. Separate from
// `PreviewState+Meter.swift`, which holds the atoms' raw series, so this card adds its values
// without editing that file.

import SwiftUI

extension PreviewState {
  /// The mockup's meter while the reply streams.
  static var meterStreaming: TokenMeter.State {
    var state = meterIdle
    state.isStreaming = true
    return state
  }

  /// The same session once the turn is done: the last call's exact rate, the dot at rest.
  static var meterIdle: TokenMeter.State {
    var state = TokenMeter.State()
    (state.sent, state.received, state.rate) = ("218.5k", "9.8k", "71")
    state.spoken = "218 thousand tokens sent, 9.8 thousand received, 71 tokens per second"
    state.sparkline = Array(series[2].samples.suffix(12))
    return state
  }

  /// The mockup's popover: a turn of four requests, streaming.
  static var tokensStreaming: TokenPopover.State {
    var state = TokenPopover.State()
    (state.heading, state.phase, state.isStreaming) = ("This turn · 4 requests", "streaming", true)
    (state.rate, state.rateUnit) = ("71", "tok/s now")
    state.rateDetail = "avg 64 tok/s · first token 800 ms · peak 77 tok/s"
    state.sparkline = series[2].samples
    state.rows = tokenRows.map {
      TokenPopover.Row(
        label: $0.label, turn: $0.values[0], session: $0.values[1], isDetail: $0.isDetail)
    }
    (state.context, state.contextShare) = ("Context · 76.4k / 200k", "38%")
    let legends = ["system 6.2k", "tools 9.8k", "AGENTS.md 11.5k", "history 48.9k"]
    state.parts = zip(bars[2].segments, legends).map {
      TokenPopover.Part(kind: $0.kind, fraction: $0.fraction, legend: $1)
    }
    state.footnote =
      "Cache hit 94% this turn · counts from the provider's usage, one ledger row per request"
    return state
  }

  /// After the turn, with the context window and its parts unknown (before the core's first
  /// `ContextBreakdown`).
  static var tokensIdle: TokenPopover.State {
    var state = tokensStreaming
    (state.heading, state.phase, state.isStreaming) = ("Last turn · 4 requests", "done", false)
    state.rateUnit = "tok/s last call"
    (state.context, state.contextShare, state.parts) = ("Context · 76.4k", "", [])
    return state
  }

  /// After the turn, fed as the live session feeds it (T37.25.2): the share and the parts as
  /// `cox_app::MeterText` formats them, each legend its label and tokens.
  static var tokensLive: TokenPopover.State {
    var state = tokensIdle
    (state.context, state.contextShare) = ("Context · 76.4k", "38% of 200k")
    let legends = ["System 6.2k", "Tools 9.8k", "Instructions 11.5k", "History 48.9k"]
    state.parts = zip(bars[2].segments, legends).map {
      TokenPopover.Part(kind: $0.kind, fraction: $0.fraction, legend: $1)
    }
    return state
  }

  /// The composer with the meter's popover open over it, as mockup screen 30 shows it.
  static var composerTokens: Composer.State {
    var state = Composer.State()
    (state.meter, state.tokens, state.isRunning) = (meterStreaming, tokensStreaming, true)
    return state
  }

  /// Room above a composer for the open token popover, which stands on its top edge.
  static let tokensRoom: CGFloat = 400
}

/// The meter as the composer's chip row shows it.
struct TokenMeterSample: View {
  let state: TokenMeter.State
  var isOpen = false

  var body: some View {
    TokenMeter(state: state, isOpen: isOpen) {}
  }
}
