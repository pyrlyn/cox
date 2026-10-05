// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Tasks tab's check (T37.29.4, DT§5.1, DS§6.4): the inspector on its Tasks tab per
// light/dark × Solid/Frosted cell (a subagent and a shell row, T37.29.8), and empty; the open
// intent a row click sends and its label per kind; its header; and a shell's output sheet
// (T37.22.6) with output and empty.

import Testing

@testable import CoxUI

@MainActor
@Suite struct TasksTabSnapshotTests {
  @Test(arguments: Variant.all) func tasksTab(_ variant: Variant) throws {
    try assertCoxSnapshot(
      TasksInspectorSample(state: PreviewState.tasks), variant, named: variant.name)
  }

  @Test func emptyTasksTab() throws {
    try assertCoxSnapshot(
      TasksInspectorSample(state: .init()), Variant.all[0], named: Variant.all[0].name)
  }

  @Test(arguments: Variant.all) func taskOutputSheet(_ variant: Variant) throws {
    try assertCoxSnapshot(
      TaskOutputSheet(title: "cargo build", output: PreviewState.taskOutput) {}, variant,
      named: variant.name)
  }

  @Test func emptyTaskOutputSheet() throws {
    try assertCoxSnapshot(
      TaskOutputSheet(title: "true", output: "") {}, Variant.all[0], named: Variant.all[0].name)
  }
}

@MainActor
@Suite struct TasksTabTests {
  /// Records what the tab sends.
  final class Log {
    var intents: [TasksTab.Intent] = []
  }

  @Test func aRowClickOpensTheTaskTranscriptByItsId() {
    let log = Log()
    let open = TasksTab.openAction("task-2", kind: .agent) { log.intents.append($0) }
    open.perform()
    #expect(open.title == "Open transcript")
    #expect(log.intents == [.open(task: "task-2")])
  }

  @Test func aShellRowOpensItsOutputUnderItsOwnGlyph() {
    let log = Log()
    let open = TasksTab.openAction("task-3", kind: .shell) { log.intents.append($0) }
    open.perform()
    #expect(open.title == "Open output")
    #expect(log.intents == [.open(task: "task-3")])
    #expect((TasksTab.Kind.agent.symbol, TasksTab.Kind.shell.symbol) == ("person.2", "terminal"))
  }

  @Test func theHeaderCountsTheTasks() {
    #expect(PreviewState.tasks.title == "Subagents & background · 3")
  }
}
