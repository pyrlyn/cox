// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// What the client seam carries for provider readiness and the provider pick (T60.4, A138): a
// status with its provider, a menu with an unusable section, a session's readiness, and a
// reopened session replacing the one a window shows.

import CoxClient
import Foundation
import Testing

@testable import CoxModel

@Test func aStatusDecodesItsProviderAndAnOldFixtureWithoutOneStillDecodes() throws {
  let json = #"{"queued":0,"model":"claude-sonnet-5","provider":"anthropic","provider_name":"Anthropic"}"#
  let status = try JSONDecoder().decode(Status.self, from: Data(json.utf8))
  #expect(status.provider == "anthropic")
  #expect(status.providerName == "Anthropic")
  let old = try JSONDecoder().decode(Status.self, from: Data(#"{"queued":1}"#.utf8))
  #expect(old.provider == nil && old.providerName == nil)
}

@Test func theFixtureMenuMarksASectionWhoseProviderIsNotUsable() throws {
  let models = FixtureModels(
    models: [
      ModelChoice(tier: .code, provider: "anthropic", id: "claude-sonnet-5"),
      ModelChoice(tier: .code, provider: "openai", id: "gpt-5"),
    ], providers: ["anthropic"])
  let sections = try models.modelMenu(cwd: "/p", usable: ["anthropic"])
  #expect(sections.map(\.provider) == ["anthropic", "openai"])
  #expect(sections.map(\.usable) == [true, false])
  #expect(sections[1].models.map(\.id) == ["gpt-5"])
}

@Test func aFixtureSessionReportsItsReadinessAndProvider() async throws {
  let session = FixtureSession(
    fixture: Fixture(batches: [], snapshot: []), provider: "openai",
    readiness: .noKey(provider: "openai"))
  #expect(session.provider() == "openai")
  let readiness = try await session.readiness()
  #expect(!readiness.isReady)
  #expect(readiness.reason == .noKey(provider: "openai"))
  #expect(readiness.message != nil)
  #expect(try await FixtureModels().readiness(cwd: "/p").reason == .noProvider)
}

@MainActor
@Test func aReopenedSessionTakesTheWindowsSlotUnderTheSameId() {
  let app = AppStore()
  let (main, popped) = (UUID(), UUID())
  let first = app.adopt(FixtureSession(fixture: Fixture(batches: [], snapshot: [])), window: main)
  _ = app.join("fixture", window: popped)
  let reopened = FixtureSession(fixture: Fixture(batches: [], snapshot: []), provider: "openai")
  let shared = app.replace("fixture", with: reopened)
  #expect(shared != nil && shared?.store !== first.store)
  // Both windows still hold the slot: one release keeps it, the second lets it go.
  app.release("fixture", window: main)
  #expect(app.isOpen("fixture"))
  app.release("fixture", window: popped)
  #expect(!app.isOpen("fixture"))
}

@MainActor
@Test func replacingASessionNoWindowShowsDoesNothing() {
  let app = AppStore()
  #expect(
    app.replace("gone", with: FixtureSession(fixture: Fixture(batches: [], snapshot: []))) == nil)
}
