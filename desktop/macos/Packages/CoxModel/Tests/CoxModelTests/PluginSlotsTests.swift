// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// SessionStore's plugin slots (T52.17's Check): a slot patch replaces only its own slot, and a
// reset of the timeline leaves the slots alone, as `cox_app::coalesce` keeps them beside it; the
// column's area reaches the session, which renders a shown panel again for it.

import CoxClient
import Testing

@testable import CoxModel

private func slot(
  _ plugin: String, _ kind: PluginSlotKind, _ text: String, visible: Bool = true
) -> PluginSlot {
  PluginSlot(
    plugin: plugin, slot: kind, view: .text(lines: [[PluginRun(text)]]), visible: visible)
}

@MainActor
@Test func aPluginSlotPatchUpdatesOnlyItsSlot() {
  let store = SessionStore(session: FixtureSession(fixture: Fixture(batches: [], snapshot: [])))
  store.apply([
    .pluginSlot(slot: slot("git", .statusLeft, "main")),
    .pluginSlot(slot: slot("git", .panel, "3 files")),
    .pluginSlot(slot: slot("todo", .statusLeft, "2 open")),
  ])

  store.apply([.pluginSlot(slot: slot("git", .statusLeft, "feature"))])

  #expect(
    store.pluginSlots[PluginSlotKey(plugin: "git", slot: .statusLeft)]?.view
      == .text(lines: [[PluginRun("feature")]]))
  #expect(
    store.pluginSlots[PluginSlotKey(plugin: "git", slot: .panel)]?.view
      == .text(lines: [[PluginRun("3 files")]]))
  #expect(
    store.pluginSlots[PluginSlotKey(plugin: "todo", slot: .statusLeft)]?.view
      == .text(lines: [[PluginRun("2 open")]]))
  #expect(store.pluginSlots.keys.map(\.plugin) == ["git", "git", "todo"])
}

@MainActor
@Test func aTimelineResetKeepsThePluginSlots() {
  let store = SessionStore(session: FixtureSession(fixture: Fixture(batches: [], snapshot: [])))
  store.apply([.pluginSlot(slot: slot("git", .statusRight, "main"))])

  store.apply([.reset(blocks: [])])

  #expect(store.pluginViews(.statusRight).map(\.plugin) == ["git"])
}

@MainActor
@Test func theColumnsAreaReachesTheSessionInCells() {
  let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []))
  let store = SessionStore(session: session)

  let (columns, rows): (UInt16, UInt16) = (120, 40)

  store.pluginArea(width: columns, height: rows)

  #expect(session.pluginAreas == [[columns, rows]])
}

@MainActor
@Test func aHiddenPanelIsNotAmongTheShownViews() {
  let store = SessionStore(session: FixtureSession(fixture: Fixture(batches: [], snapshot: [])))
  store.apply([
    .pluginSlot(slot: slot("git", .panel, "3 files", visible: false)),
    .pluginSlot(slot: slot("todo", .panel, "2 open")),
  ])

  #expect(store.pluginViews(.panel).map(\.plugin) == ["todo"])
}
