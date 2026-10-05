// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The session toolbar's figures (T37.22.5): the title and project from the session's entry, the
// branch from its Info, the cost and the context share the meter formatted with the ring's fill
// from the split; before the first usage patch, a zero cost and an unknown share; an external
// agent's session names its agent in the model chip and has no cost (T52.8).

import CoxClient
import Foundation
import Testing

@testable import CoxModel

@MainActor
@Test func theFixturesMeterFillsTheCostAndContextPill() async throws {
  let url = try #require(fixtures.first { $0.lastPathComponent == "read-and-reply.json" })
  let session = try await FixtureCoreClient(fixture: try Fixture(contentsOf: url))
    .open(OpenSession(cwd: "/", theme: "base16-ocean.dark"))
  let store = SessionStore(session: session)
  let before = ToolbarState(usage: store.usage, entry: nil, info: nil)
  #expect(before.cost == "$0.00")
  #expect(before.context == "–")
  #expect(before.contextFraction == 0)

  await store.run()

  let usage = try #require(store.usage)
  let figures = ToolbarState(usage: usage, entry: nil, info: nil)
  #expect(figures.cost == usd(usage.session.costUsd))
  // `0.4% of 1M` in the meter.
  #expect(figures.context == "0.4%")
  #expect(figures.contextFraction > 0 && figures.contextFraction < 0.01)
}

@Test func theEntryAndInfoNameTheSessionAndWhereItRuns() {
  let entry = SessionEntry(id: "s", title: "Add retry jitter", cwd: "/src/cox/wt")
  let tree = Linked(path: "/src/cox/wt", branch: "wt/retry", bytes: 0)
  let info = Info(session: "s", cwd: "/src/cox/wt", worktree: tree)
  let figures = ToolbarState(
    usage: nil, entry: (entry, Project(root: "/src/cox", name: "cox")), info: info)
  #expect(figures.title == "Add retry jitter")
  #expect(figures.project == "cox")
  #expect(figures.branch == "wt/retry")

  let fresh = ToolbarState(usage: nil, entry: nil, info: Info(session: "s", cwd: "/src/acme-web"))
  #expect(fresh.title == "Untitled session")
  #expect(fresh.project == "acme-web")
  #expect(fresh.branch == nil)
}

@Test func anExternalAgentsSessionNamesItsAgentAndHasNoCost() {
  let entry = SessionEntry(id: "s", title: "Refactor checkout form", agent: "claude")
  let agents = [AgentChoice.cox, AgentChoice(name: "claude", label: "Claude Agent")]
  let figures = ToolbarState(
    usage: nil, entry: (entry, Project(root: "/src/web", name: "web")), info: nil, agents: agents)
  #expect(figures.model == "Claude Agent · ACP")
  #expect(figures.cost == "—")

  let unlisted = ToolbarState(
    usage: nil, entry: (entry, Project(root: "/src/web", name: "web")), info: nil)
  #expect(unlisted.model == "claude · ACP")
  #expect(ToolbarState(usage: nil, entry: nil, info: nil).model == nil)
}
