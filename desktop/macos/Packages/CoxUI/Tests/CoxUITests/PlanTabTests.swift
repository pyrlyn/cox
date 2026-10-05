// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Plan tab's check (T37.29.2, DT§5.1, DS§6.4): the inspector on its Plan tab per
// light/dark × Solid/Frosted cell, and empty; its header.

import Testing

@testable import CoxUI

@MainActor
@Suite struct PlanTabSnapshotTests {
  @Test(arguments: Variant.all) func planTab(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PlanInspectorSample(state: PreviewState.plan), variant, named: variant.name)
  }

  @Test func emptyPlanTab() throws {
    try assertCoxSnapshot(
      PlanInspectorSample(state: .init()), Variant.all[0], named: Variant.all[0].name)
  }
}

@MainActor
@Suite struct PlanTabTests {
  @Test func theHeaderCountsTheDoneStepsOfAll() {
    #expect(PreviewState.plan.title == "Plan · 2 of 5")
  }
}
