// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The toolbar's model popover (T37.22.6): the core's sections as they come (T58.4.13; which
// tiers and models they list is `cox_app::models`' tests), the running one marked, each by the
// catalog name the core sent, and a pick sends the TUI's `/model <tier> <id>` switch.

import CoxClient
import Testing

@testable import CoxModel

private let sections = [
  ModelSection(
    tier: .code, title: "Code",
    models: [
      MenuModel(
        id: "claude-sonnet-5", displayName: "Claude Sonnet 5", shortName: "Sonnet 5",
        efforts: "low · high"),
      MenuModel(id: "claude-haiku-4-5"),
    ]),
  ModelSection(tier: .think, title: "Think", models: [MenuModel(id: "claude-fable-5-1")]),
]

@Test func theCoresSectionsShowWithTheRunningOneMarked() {
  let menu = ModelMenu(sections: sections, status: Status(model: "claude-sonnet-5", effort: .high))
  #expect(menu.sections.map(\.title) == ["Code", "Think"])
  #expect(menu.sections[0].rows.map(\.model) == ["claude-sonnet-5", "claude-haiku-4-5"])
  // The core's short name, else the id (A111, A129).
  #expect(menu.sections[0].rows.map(\.name) == ["Sonnet 5", "claude-haiku-4-5"])
  #expect(menu.sections[0].rows.map(\.detail) == ["low · high", ""])
  #expect(menu.sections[0].rows.map(\.isSelected) == [true, false])
  #expect(menu.sections[1].rows.map(\.model) == ["claude-fable-5-1"])
}

@Test func aPickSwitchesTheRowsTierToItsModel() {
  let menu = ModelMenu(sections: sections, status: Status())
  let fable = menu.sections[1].rows[0]
  #expect(menu.pick(fable.id) == .switchModel(tier: .think, model: "claude-fable-5-1"))
  #expect(menu.pick("code/nope") == nil)
}
