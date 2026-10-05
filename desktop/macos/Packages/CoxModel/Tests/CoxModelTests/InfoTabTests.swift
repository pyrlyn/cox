// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Info tab's state from cox-app's `Info` (T37.29.5): the core's facts fill the tab as they
// arrive, read through SessionStore from the fixture session. What the facts say (`~`,
// `detached`, the key counts) is `cox_app::info`'s test (T58.4.20).

import CoxClient
import Testing

@testable import CoxModel

private let info = Info(
  session: "01J9ZK4Q", cwd: "/Users/me/GitHub/cox",
  rollout: "/Users/me/.cox/sessions/01J9ZK4Q.jsonl",
  facts: [
    Fact(label: "Session", value: "01J9ZK4Q"),
    Fact(label: "Worktree", value: "~/GitHub/_worktrees/cox-t1"),
    Fact(label: "Branch", value: "t1", detail: true),
  ],
  configFacts: [
    Fact(label: "user", value: "1 key"),
    Fact(label: "~/.cox/config.toml", detail: true),
  ])

@MainActor
@Test func theStoreFillsTheInfoTabFromTheSession() async throws {
  let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []), info: info)
  let tab = try await SessionStore(session: session).infoTab()
  #expect(tab == InfoTabState(info))
}

@Test func theTabListsTheCoreFactsInTheirOrder() {
  let tab = InfoTabState(info)
  #expect(
    tab.session == [
      .init(label: "Session", values: ["01J9ZK4Q"]),
      .init(label: "Worktree", values: ["~/GitHub/_worktrees/cox-t1"]),
      .init(label: "Branch", values: ["t1"], isDetail: true),
    ])
  #expect(
    tab.config == [
      .init(label: "user", values: ["1 key"]),
      .init(label: "~/.cox/config.toml", values: [], isDetail: true),
    ])
}
