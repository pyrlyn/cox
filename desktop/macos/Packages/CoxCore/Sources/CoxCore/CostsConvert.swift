// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Context tab's cost history from cox-ffi into CoxClient's (T37.29.3.2), apart from
// `Convert.swift`'s timeline so each file stays one concern. Field for field; nothing is
// decided here.

import CoxClient
import CoxFFIBindings

extension CoxClient.TurnCosts {
  init(_ costs: CoxFFIBindings.TurnCosts) {
    self.init(
      columns: costs.columns, rows: costs.rows.map { CoxClient.CostRow($0) },
      total: CoxClient.CostRow(costs.total), project: costs.project,
      budget: costs.budget.map { CoxClient.BudgetRow($0) })
  }
}

extension CoxClient.CostRow {
  init(_ row: CoxFFIBindings.CostRow) {
    self.init(label: row.label, values: row.values, detail: row.detail)
  }
}

extension CoxClient.BudgetRow {
  init(_ row: CoxFFIBindings.BudgetRow) {
    self.init(label: row.label, text: row.text, fraction: row.fraction)
  }
}
