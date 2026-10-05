// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Settings sidebar's search (T37.45.1): a query keeps only the pages and fields whose label
// or dotted config key holds it, ignoring case, and an empty query keeps everything.

import CoxClient
import Testing

@testable import CoxModel

@MainActor
@Test func aQueryKeepsOnlyMatchingPagesAndFields() async throws {
  let (store, _) = await loadedStore()
  store.filter = "MODEL"
  #expect(store.sections.map(\.group) == [.models])
  #expect(store.sections.flatMap(\.settings).map(\.key) == ["tiers.code.model"])

  // The label `Base url` matches where the key `base_url` does not.
  store.filter = " base url "
  let models = try #require(store.sections.first)
  #expect(models.settings.map(\.key) == ["providers.anthropic.base_url"])
  #expect(store.tables(in: models).map(\.provider) == ["anthropic"])

  // A key's table prefix keeps the whole page.
  store.filter = "appearance"
  #expect(store.sections.map(\.group) == [.appearance])
  #expect(store.sections.first?.settings.count == 2)

  store.filter = "no such setting"
  #expect(store.sections.isEmpty)

  store.filter = ""
  #expect(store.sections.map(\.group) == [.models, .budget, .appearance])
}
