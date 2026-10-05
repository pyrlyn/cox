// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// "Ask cox in <project>" (T51.17): the intent opens a new session in the project, then sends
// the prompt as the composer's Send, and the session stays held until a window joins it.

import CoxClient
import Foundation
import Synchronization
import Testing

@testable import CoxModel

private final class OpeningCore: CoreClient {
  let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []))
  let requests = Mutex<[OpenSession]>([])

  func open(_ request: OpenSession) async throws -> any SessionClient {
    requests.withLock { $0.append(request) }
    return session
  }
}

@MainActor
@Test func askingOpensASessionInTheProjectThenSendsThePrompt() async throws {
  let (core, registry, token) = (OpeningCore(), AppStore(), UUID())
  let ask = AskCox(project: "/work/cox", prompt: "Why does the build fail?")
  let shared = try await ask.run(on: core, registry: registry, token: token, theme: "t")
  #expect(core.requests.withLock { $0 } == [OpenSession(cwd: "/work/cox", theme: "t")])
  #expect(core.session.sent == [.send(text: "Why does the build fail?", attachments: [])])
  #expect(shared.store.session === core.session)
}

@MainActor
@Test func theAskedSessionStaysOpenUntilItsWindowJoins() async throws {
  let (core, registry, token, window) = (OpeningCore(), AppStore(), UUID(), UUID())
  _ = try await AskCox(project: "/work/cox", prompt: "hi").run(
    on: core, registry: registry, token: token, theme: "t")
  #expect(registry.isOpen("fixture"))
  _ = registry.join("fixture", window: window)
  registry.release("fixture", window: token)
  #expect(registry.isOpen("fixture"))
}
