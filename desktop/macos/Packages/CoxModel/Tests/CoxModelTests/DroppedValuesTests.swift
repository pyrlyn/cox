// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Dropped project values through SettingsStore (T37.30.4) over the fixture client: a budget the
// project raised shows on the budget page with its reason, and on no other page.

import CoxClient
import Testing

@testable import CoxModel

@MainActor
@Suite struct DroppedValuesTests {
  @Test func aRaisedBudgetIsListedOnTheBudgetPageWithItsReason() async {
    let reason = "A project may not raise a budget above your own"
    let view = SettingsView(
      settings: [], userFile: "/u/config.toml", projectFile: "/p/.cox/config.toml",
      dropped: [
        Dropped(
          key: "budget.session_usd", value: "999", kept: "5", reason: reason, group: .budget,
          change: "999 → 5")
      ])
    let store = SettingsStore(
      client: FixtureSettingsClient(view: view), secrets: MemorySecretStore(), cwd: "/p")
    await store.load()

    #expect(
      store.dropped(in: .budget) == [
        DroppedRow(key: "budget.session_usd", reason: reason, change: "999 → 5")
      ])
    #expect(store.dropped(in: .general).isEmpty)
  }
}
