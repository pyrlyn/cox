// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The inspector's Tasks tab (T37.29.4, DT§5.1): the session's subagents and background calls,
// read off its `task` blocks — label, tier, state and cost. Here, not in CoxUI, because these
// decide what the tab shows (DS§1); the app copies them into CoxUI's `TasksTab.Item` field for
// field, and a click hands `id` back to `open(task:)` (T37.29.6), which the core resolves to a
// subagent's session or a shell's output.

import CoxClient
import Foundation

public struct TaskRow: Identifiable, Equatable, Sendable {
  /// The core's reading of the task (T58.4.25); the task card reads the same value.
  public typealias State = TaskState

  /// The core's task id, which opening the task's transcript names.
  public let id: String
  public let label: String
  /// The tier it runs on, as the transcript's task card names it: `cheap`.
  public let tier: String
  /// A subagent or a background shell, which the tab labels and a click opens differently.
  public let kind: TaskKind
  public let state: State
  /// `$0.07`, as the TUI writes a cost; `nil` while it runs, before the core has one.
  public let cost: String?
}

extension SessionStore {
  /// Every task block in timeline order.
  public var tasks: [TaskRow] {
    blocks.values.compactMap { block in
      guard
        case .task(let task, let label, let tier, let done, let costUsd, _, let state, let kind) =
          block.kind
      else { return nil }
      return TaskRow(
        id: task, label: label, tier: tier.rawValue, kind: kind, state: state,
        cost: done ? usd(costUsd) : nil)
    }
  }

  /// What the Tasks tab's `.open(task:)` shows: the subagent's child session or the shell's
  /// archived output, as the core finds them; `nil` while there is nothing to open.
  public func open(task: String) throws -> TaskTarget? {
    try session.openTask(task)
  }
}
