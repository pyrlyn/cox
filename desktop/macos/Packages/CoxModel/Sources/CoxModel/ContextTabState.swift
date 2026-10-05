// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The inspector's Context tab (T37.29.3.1, DT§5.1 Context & Cost): the window's split and the
// turn's cache hit from the token meter's latest `UsageView`, and "Compact now". Here, not in
// CoxUI, because the meter's figures decide what the tab shows (DS§1); the app copies them into
// `ContextTab.State` field for field. `CostHistoryState` is the tab's cost by turn, read from
// the ledger when the tab asks (T37.29.3.2), with the project's spend as its footnote
// (T37.29.3.3). `[desktop.context] cache_hit` picks the turn's or the session's cache hit (A104);
// "Compact now" waits while the meter's turn runs (A105). The budget caps against the spend ride
// with the cost history (T37.29.3.4).

import CoxClient

public struct ContextTabState: Equatable, Sendable {
  public var split = ContextSplit()
  /// `94% this turn` or `88% this session`, as `scope` picks; empty before anything was sent.
  public var cacheHit = ""
  /// The meter's turn has not ended, so "Compact now" waits for it (A105).
  public var turnRunning = false

  public init() {}

  /// Empty until the first `usage` patch.
  public init(_ usage: UsageView?, cacheHit scope: CacheHitScope = .turn) {
    guard let usage else { return }
    let text = usage.text
    split = ContextSplit(text)
    cacheHit = scope == .turn ? text.cacheHit : text.cacheHitSession
    turnRunning = usage.turn.map { !$0.done } ?? false
  }
}

/// `[desktop.context] cache_hit`, spelled as Rust stores it (A104).
public enum CacheHitScope: String, Equatable, Sendable, Decodable {
  /// The last turn's cache reads over what it sent.
  case turn
  /// Every call's so far.
  case session
}

extension SettingsStore {
  /// `[desktop.context] cache_hit` from the loaded view; per turn before the first load or when
  /// the key holds something else.
  public var cacheHitScope: CacheHitScope {
    view.flatMap { SectionRows($0.settings, "desktop.context").decode("cache_hit") } ?? .turn
  }
}

/// The tab's "Cost by turn": cox-app's `TurnCosts` as `KeyValueGrid` rows, the session total
/// last. Empty, so the section hides, before the ledger has a row.
public struct CostHistoryState: Equatable, Sendable {
  /// `KeyValueGrid.Row`.
  public struct Row: Equatable, Sendable {
    public var label: String
    public var values: [String]
    public var isDetail = false
  }

  /// `In`, `Out`, `Cache r/w`, `$`.
  public var columns: [String] = []
  /// A row per turn, its subagents as detail rows, then `Session`.
  public var rows: [Row] = []
  /// `Project cox today: $3.18 · this week: $21.40. …`, shown even before this session spent.
  public var footnote = ""
  /// The session's and the month's spend against their caps, shown even before this session
  /// spent (T37.29.3.4).
  public var budget: [BudgetRow] = []

  public init() {}

  public init(_ costs: TurnCosts) {
    (footnote, budget) = (costs.project, costs.budget)
    guard !costs.rows.isEmpty else { return }
    let row = { (cost: CostRow) in
      Row(label: cost.label, values: cost.values, isDetail: cost.detail)
    }
    (columns, rows) = (costs.columns, costs.rows.map(row) + [row(costs.total)])
  }
}

extension SessionStore {
  /// The cost by turn, read from the core when the tab asks (T37.29.3.2).
  public func costHistory() async throws -> CostHistoryState {
    CostHistoryState(try await session.turnCosts())
  }

  /// The Context tab over the meter's latest figures; it follows every `usage` patch.
  public var contextTab: ContextTabState { contextTab(cacheHit: .turn) }

  /// As `contextTab`, with the cache hit `SettingsStore.cacheHitScope` picks.
  public func contextTab(cacheHit scope: CacheHitScope) -> ContextTabState {
    ContextTabState(usage, cacheHit: scope)
  }

  /// The tab's "Compact now": the same manual compaction as `/compact` with no focus.
  public func compactNow() async throws {
    _ = try await send(.compact(focus: nil))
  }
}
