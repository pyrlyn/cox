// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Context tab's cost history (T37.29.3.2, DT§5.1), field for field as cox-ffi exports
// `cox_app::TurnCosts`: the ledger's rows by turn, subagents under their turn, the session
// total, the project's spend today and this week and the budget caps against the spend, every figure formatted by the core.
// Separate from the timeline because it answers a call, not a patch.

public struct TurnCosts: Equatable, Sendable {
  /// The value columns' headers: `In`, `Out`, `Cache r/w`, `$`.
  public var columns: [String]
  /// A row per turn in order, each turn's subagents under it as detail rows.
  public var rows: [CostRow]
  /// `Session`: every row summed.
  public var total: CostRow
  /// The footnote: `Project cox today: $3.18 · this week: $21.40. …` (T37.29.3.3).
  public var project: String
  /// The session's and the month's spend against `[budget]`'s caps, session first (T37.29.3.4).
  public var budget: [BudgetRow]

  public init(
    columns: [String] = [], rows: [CostRow] = [], total: CostRow = CostRow(), project: String = "",
    budget: [BudgetRow] = []
  ) {
    (self.columns, self.rows, self.total, self.project, self.budget) =
      (columns, rows, total, project, budget)
  }
}

/// One cap against its spend: `Session`, `$0.42 of $5.00`, a gauge at 8%.
public struct BudgetRow: Equatable, Sendable {
  public var label: String
  /// `$0.42 of $5.00`, or `$0.42` alone when the cap is not a usable number.
  public var text: String
  /// The spend over the cap, 0...1; nil without a usable cap.
  public var fraction: Double?

  public init(label: String = "", text: String = "", fraction: Double? = nil) {
    (self.label, self.text, self.fraction) = (label, text, fraction)
  }
}

/// One line of the grid: `1 · code` (a subagent's `explore`), then a value per column.
public struct CostRow: Equatable, Sendable {
  public var label: String
  public var values: [String]
  /// A subagent's row, drawn indented under its turn.
  public var detail: Bool

  public init(label: String = "", values: [String] = [], detail: Bool = false) {
    (self.label, self.values, self.detail) = (label, values, detail)
  }
}
