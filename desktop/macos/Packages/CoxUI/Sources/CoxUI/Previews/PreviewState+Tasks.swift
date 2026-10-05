// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the inspector's Tasks tab (T37.29.4): the mockup's subagents and
// background call — one running, one done, one failed. Separate from `PreviewState+Inspector.swift`
// so the inspector's tabs, built in parallel, add their fixtures without editing one file.

import SwiftUI

extension PreviewState {
  static let taskItems: [TasksTab.Item] = [
    .init(id: "task-1", label: "reviewer", tier: "cheap", kind: .agent, state: .running),
    .init(
      id: "task-2", label: "test-writer", tier: "code", kind: .agent, state: .succeeded,
      cost: "$0.07"),
    .init(
      id: "task-3", label: "bash: cargo nextest run", tier: "cheap", kind: .shell, state: .failed,
      cost: "$0.00"),
  ]

  /// The mockup's Tasks tab.
  static let tasks = TasksTab.State(items: taskItems)

  /// A finished shell's archived output, as its sheet shows it (T37.22.6).
  static let taskOutput = """
       Compiling cox-app v0.1.2
       Compiling cox-ffi v0.1.2
        Finished `dev` profile [unoptimized + debuginfo] target(s) in 9.84s
    """
}

/// The inspector on its Tasks tab, as tall as the smallest window.
struct TasksInspectorSample: View {
  let state: TasksTab.State

  var body: some View {
    Inspector(selection: .tasks, content: TasksTab(state: state) { _ in }) { _ in }
      .frame(height: Size.windowMinHeight)
  }
}
