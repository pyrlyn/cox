// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The model popover grouped by provider and the provider pick it sends (T60.7, A139, DT§5.3): a
// model of another provider is a provider switch, one of the session's own is the model switch,
// a provider with no key does not pick, a remote session cannot change provider, and the session
// a switch reopens takes the window's slot with its draft.

import CoxClient
import Foundation
import Testing

@testable import CoxModel

private let sections = [
  ModelSection(
    tier: .code, title: "Code", provider: "anthropic", usable: true,
    models: [
      MenuModel(id: "claude-sonnet-5", shortName: "Sonnet 5", efforts: "low · high"),
      MenuModel(id: "claude-haiku-4-5"),
    ]),
  ModelSection(
    tier: .think, title: "Think", provider: "deepseek", usable: true,
    models: [MenuModel(id: "deepseek-r2")]),
  ModelSection(
    tier: .code, title: "OpenAI", provider: "openai", usable: true,
    models: [MenuModel(id: "gpt-5-5", shortName: "GPT-5.5"), MenuModel(id: "claude-sonnet-5")]),
  ModelSection(
    tier: .code, title: "Mistral", provider: "mistral", usable: false,
    models: [MenuModel(id: "mistral-large")]),
]

private let running = Status(model: "claude-sonnet-5", provider: "anthropic")

@Test func theSectionsKeepTheirProviderAndWhetherItCanAnswer() {
  let menu = ModelMenu(sections: sections, status: running)
  #expect(menu.sections.map(\.provider) == ["anthropic", "deepseek", "openai", "mistral"])
  #expect(menu.sections.map(\.usable) == [true, true, true, false])
  #expect(menu.sections.map(\.isEnabled) == [true, true, true, false])
}

@Test func theSameModelIdUnderTwoProvidersIsTwoRowsAndOnlyTheSessionsOwnIsMarked() {
  let menu = ModelMenu(sections: sections, status: running)
  let ids = menu.sections.flatMap(\.rows).map(\.id)
  #expect(Set(ids).count == ids.count)
  #expect(menu.sections[0].rows[0].isSelected)
  #expect(!menu.sections[2].rows[1].isSelected)
}

@Test func aModelOfAnotherProviderPicksAProviderSwitch() {
  let menu = ModelMenu(sections: sections, status: running)
  let gpt = menu.sections[2].rows[0]
  #expect(
    menu.pick(gpt.id) == .switchProvider(provider: "openai", model: "gpt-5-5", makeDefault: false))
}

@Test func aModelOfTheSessionsOwnProviderPicksTheModelSwitch() {
  let menu = ModelMenu(sections: sections, status: running)
  let haiku = menu.sections[0].rows[1]
  #expect(menu.pick(haiku.id) == .switchModel(tier: .code, model: "claude-haiku-4-5"))
}

@Test func aThinkTiersOwnProviderIsATierSwitchNotAProviderSwitch() {
  let menu = ModelMenu(sections: sections, status: running)
  let think = menu.sections[1].rows[0]
  #expect(menu.pick(think.id) == .switchModel(tier: .think, model: "deepseek-r2"))
}

@Test func aSessionWhoseProviderIsNotKnownYetPicksTheModelSwitch() {
  let menu = ModelMenu(sections: sections, status: Status(model: "claude-sonnet-5"))
  let gpt = menu.sections[2].rows[0]
  #expect(menu.pick(gpt.id) == .switchModel(tier: .code, model: "gpt-5-5"))
}

@Test func aProviderWithoutAKeyDoesNotPick() {
  let menu = ModelMenu(sections: sections, status: running)
  let mistral = menu.sections[3].rows[0]
  #expect(menu.pick(mistral.id) == nil)
}

@Test func aRemoteSessionListsOtherProvidersDisabledWithTheReason() {
  let menu = ModelMenu(sections: sections, status: running, canSwitchProvider: false)
  #expect(menu.sections[2].unavailable != nil && menu.sections[2].usable)
  #expect(menu.pick(menu.sections[2].rows[0].id) == nil)
  // Its own provider's models still switch.
  #expect(menu.sections[0].unavailable == nil)
  #expect(menu.pick(menu.sections[0].rows[1].id) != nil)
}

@MainActor
@Test func theSessionASwitchReopensTakesTheWindowsSlotWithItsDraft() async throws {
  let app = AppStore()
  let window = UUID()
  let first = FixtureSession(fixture: Fixture(batches: [], snapshot: []), provider: "anthropic")
  let reopened = FixtureSession(fixture: Fixture(batches: [], snapshot: []), provider: "openai")
  first.reopens(as: reopened)
  let before = app.adopt(first, window: window)
  before.composer.edit("fix the retry loop")

  let after = try await app.send(
    .switchProvider(provider: "openai", model: "gpt-5-5", makeDefault: false),
    to: before.store)

  // A fresh store over the reopened session sits in the same slot, and the next window to join
  // gets it, not the old one.
  let shared = try #require(after)
  #expect(shared.store !== before.store && shared.store.session.provider() == "openai")
  #expect(app.join("fixture", window: UUID())?.store === shared.store)
  #expect(shared.composer.text == "fix the retry loop")
  #expect(first.isClosed)
  app.release("fixture", window: window)
}

@MainActor
@Test func aModelSwitchLeavesTheStoresAlone() async throws {
  let app = AppStore()
  let before = app.adopt(
    FixtureSession(fixture: Fixture(batches: [], snapshot: [])), window: UUID())
  let after = try await app.send(
    .switchModel(tier: .code, model: "claude-haiku-4-5"), to: before.store)
  #expect(after == nil)
  #expect(app.join("fixture", window: UUID())?.store === before.store)
}
