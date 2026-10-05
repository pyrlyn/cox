// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// T52.7: the New-session picker lists cox and the external agents, refuses one that cannot
// start, and carries the pick into the `OpenSession` the core opens.

import CoxClient
import Synchronization
import Testing

@testable import CoxModel

private let claude = AgentChoice(
  name: "claude", label: "Claude Agent", origin: "user config",
  launch: "claude-agent-acp --hide-claude-auth · key ANTHROPIC_API_KEY · network")
private let codex = AgentChoice(
  name: "codex", label: "Codex", origin: "user config", unavailable: "CODEX_API_KEY is not set")

private final class Changing: AgentsClient {
  let rows: Mutex<[AgentChoice]>
  init(_ rows: [AgentChoice]) { self.rows = Mutex(rows) }
  func agents(cwd: String) async -> [AgentChoice] { rows.withLock { $0 } }
}

private struct Failing: AgentsClient {
  struct Broken: Error {}
  func agents(cwd: String) async throws -> [AgentChoice] { throw Broken() }
}

@MainActor
@Test func agentPickerCarriesThePickedAgentIntoTheOpenRequest() async {
  let picker = AgentPicker(client: FixtureAgents([.cox, claude, codex]))
  await picker.load(cwd: "/work/cox")
  #expect(picker.choices.map(\.label) == ["cox", "Claude Agent", "Codex"])
  #expect(picker.request(cwd: "/work/cox", theme: "t") == OpenSession(cwd: "/work/cox", theme: "t"))
  #expect(picker.choose("claude"))
  #expect(picker.selection.label == "Claude Agent")
  #expect(
    picker.request(cwd: "/work/cox", theme: "t")
      == OpenSession(cwd: "/work/cox", theme: "t", agent: "claude"))
}

@MainActor
@Test func agentPickerRefusesAnAgentThatCannotStart() async {
  let picker = AgentPicker(client: FixtureAgents([.cox, claude, codex]))
  await picker.load(cwd: "/work/cox")
  #expect(!picker.choose("codex"))
  #expect(!picker.choose("gone"))
  #expect(picker.chosen == nil)
}

@MainActor
@Test func agentPickerFallsBackToCoxWhenThePickCanNoLongerStart() async {
  let client = Changing([.cox, claude])
  let picker = AgentPicker(client: client)
  await picker.load(cwd: "/a")
  #expect(picker.choose("claude"))
  let unkeyed = AgentChoice(name: "claude", label: "Claude Agent", unavailable: "no key")
  client.rows.withLock { $0 = [.cox, unkeyed] }
  await picker.load(cwd: "/a")
  #expect(picker.chosen == nil)
  let failing = AgentPicker(client: Failing())
  await failing.load(cwd: "/a")
  #expect(failing.choices == [.cox])
  #expect(failing.failure != nil)
}
