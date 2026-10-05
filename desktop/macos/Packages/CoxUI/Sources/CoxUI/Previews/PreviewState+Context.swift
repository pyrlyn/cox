// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the inspector's Context tab (T37.29.3.1): a turn's split of a 200k
// window as `cox_app::MeterText` formats it, the same split with the window unknown, and mockup
// 10's cost by turn and project footnote as `cox_app::TurnCosts` formats them (T37.29.3.2,
// T37.29.3.3), the budget caps against the spend (T37.29.3.4), and the session's cache hit while a turn runs (A104, A105). Separate
// from `PreviewState+Inspector.swift` so the inspector's tabs, built in parallel, add their
// fixtures without editing one file.

import SwiftUI

extension PreviewState {
  /// Mockup screen 10's window: the bar's `turn` mix of 200k, with the parts' tokens.
  static var contextTab: ContextTab.State {
    var state = ContextTab.State(context: "Context · 76.4k", share: "38% of 200k")
    let figures = [("System", "6.2k"), ("Tools", "9.8k"), ("Instructions", "11.5k")]
    state.parts = zip(bars[2].segments, figures + [("History", "48.9k")]).map {
      ContextTab.Part(kind: $0.kind, fraction: $0.fraction, label: $1.0, tokens: $1.1)
    }
    (state.free, state.cacheHit) = ("123.6k", "94% this turn")
    return state
  }

  /// Mockup screen 10's cost by turn: two turns, a subagent under the first, the session total,
  /// and the project's spend under it.
  static var contextCosts: ContextTab.State {
    var state = contextTab
    state.costColumns = ["In", "Out", "Cache r/w", "$"]
    state.costs = [
      .init(label: "1 · code", values: ["31.4k", "2.2k", "28.0k/3.1k", "0.29"]),
      .init(label: "explore", values: ["9.8k", "600", "0/9.8k", "0.03"], isDetail: true),
      .init(label: "2 · code", values: ["16.8k", "900", "15.9k/800", "0.10"]),
      .init(label: "Session", values: ["58.0k", "3.7k", "43.9k/13.7k", "0.42"]),
    ]
    state.footnote =
      "Project cox today: $3.18 · this week: $21.40. Every number is a row in the cost ledger."
    return state
  }

  /// The budget under the cost by turn: the session against its cap and the month against its own,
  /// as `cox_app::TurnCosts` formats them (T37.29.3.4).
  static var contextBudget: ContextTab.State {
    var state = contextCosts
    state.budget = [
      .init(label: "Session", text: "$0.42 of $5.00", fraction: 0.084),
      .init(label: "This month", text: "$21.40 of $100.00", fraction: 0.214),
    ]
    return state
  }

  /// A cap that is not a usable number: the month's row is the spend alone, with no gauge.
  static var contextBudgetWithoutACap: ContextTab.State {
    var state = contextBudget
    state.budget[1] = .init(label: "This month", text: "$21.40")
    return state
  }

  /// The session's cache hit, as `[desktop.context] cache_hit = "session"` picks, while a turn
  /// runs, so "Compact now" is disabled.
  static var contextRunning: ContextTab.State {
    var state = contextTab
    (state.cacheHit, state.turnRunning) = ("88% this session", true)
    return state
  }

  /// A model whose window the catalog does not know: the bar is the whole context, nothing free.
  static var contextNoWindow: ContextTab.State {
    var state = contextTab
    let whole = state.parts.reduce(0) { $0 + $1.fraction }
    for index in state.parts.indices { state.parts[index].fraction /= whole }
    (state.share, state.free) = ("", "")
    return state
  }
}

/// The inspector on its Context tab, as tall as the smallest window.
struct ContextInspectorSample: View {
  let state: ContextTab.State

  var body: some View {
    Inspector(selection: .context, content: ContextTab(state: state) { _ in }) { _ in }
      .frame(height: Size.windowMinHeight)
  }
}
