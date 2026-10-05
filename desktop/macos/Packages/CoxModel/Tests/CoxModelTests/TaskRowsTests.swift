// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Tasks tab's rows through SessionStore (T37.29.4): task blocks become rows in timeline
// order with their kind, state and cost, and every other block is left out; opening a row asks
// the core what it opens (T37.29.6).

import CoxClient
import Testing

@testable import CoxModel

@MainActor
@Suite struct TaskRowsTests {
  @Test func taskBlocksBecomeRowsWithStateAndCostInTimelineOrder() {
    let blocks = [
      Block(
        id: "task:a", turn: 1,
        kind: .task(
          task: "a", label: "reviewer", tier: .cheap, done: false, costUsd: 0, exitCode: nil,
          state: .running, kind: .agent)),
      Block(id: "notice:1", turn: 1, kind: .notice(level: .info, text: "hi")),
      Block(
        id: "task:b", turn: 1,
        kind: .task(
          task: "b", label: "test-writer", tier: .code, done: true, costUsd: 0.071, exitCode: nil,
          state: .succeeded, kind: .agent)),
      Block(
        id: "task:c", turn: 2,
        kind: .task(
          task: "c", label: "bash: cargo test", tier: .cheap, done: true, costUsd: 0,
          exitCode: 101, state: .failed, kind: .shell)),
    ]
    let store = SessionStore(session: FixtureSession(fixture: Fixture(batches: [], snapshot: [])))
    store.apply([.reset(blocks: blocks)])

    #expect(
      store.tasks == [
        TaskRow(
          id: "a", label: "reviewer", tier: "cheap", kind: .agent, state: .running, cost: nil),
        TaskRow(
          id: "b", label: "test-writer", tier: "code", kind: .agent, state: .succeeded,
          cost: "$0.07"),
        TaskRow(
          id: "c", label: "bash: cargo test", tier: "cheap", kind: .shell, state: .failed,
          cost: "$0.00"),
      ])
  }

  @Test func openingATaskResolvesToTheChildSessionOrTheShellOutput() throws {
    let session = FixtureSession(
      fixture: Fixture(batches: [], snapshot: []),
      tasks: ["a": .transcript(session: "child"), "c": .output(archive: "out")])
    let store = SessionStore(session: session)

    #expect(try store.open(task: "a") == .transcript(session: "child"))
    #expect(try store.open(task: "c") == .output(archive: "out"))
    #expect(try store.open(task: "gone") == nil)
  }
}
