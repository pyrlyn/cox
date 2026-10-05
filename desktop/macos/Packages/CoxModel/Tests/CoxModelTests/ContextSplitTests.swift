// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// T37.25.2's check (A98): the context split the core sends with each usage view reaches the
// store the token popover reads, from every recorded fixture. Separate from SessionStoreTests so
// this card adds its claim without editing that suite.

import CoxClient
import Foundation
import Testing

@testable import CoxModel

@MainActor
@Test(arguments: fixtures)
func aFixturesContextSplitReachesThePopoverState(url: URL) async throws {
  let session = FixtureSession(fixture: try Fixture(contentsOf: url))
  let store = SessionStore(session: session)

  await store.run()

  let text = try #require(store.usage?.text)
  #expect(text.contextShare.hasSuffix(" of 1M"))
  #expect(text.contextParts.map(\.kind) == ["system", "tools", "instructions", "history"])
  #expect(text.contextParts.allSatisfy { !$0.label.isEmpty && !$0.tokens.isEmpty })
  #expect(text.contextParts.map(\.share).reduce(0, +) > 0)
}
