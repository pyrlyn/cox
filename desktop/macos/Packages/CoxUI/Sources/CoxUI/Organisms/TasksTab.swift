// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `TasksTab` (DS§6.4 row `TasksTab`, the mockup's Tasks `.ib`; DT§5.1 Tasks): the inspector's
// fourth tab — the subagents and background calls this session started, each with its tier,
// state and cost. Separate so the `Inspector` frame stays a slot and each tab is its own view,
// fed plain values the app copies from the core (T37.29.4).

import SwiftUI

/// One `InspectorSection` of task rows, newest last as the core started them; with none, one
/// quiet line. A row click asks the app to open that task's transcript, or a shell's output.
public struct TasksTab: View {
  /// What the tab lists, formatted by the core.
  public struct State: Equatable, Sendable {
    public var items: [Item] = []

    public init(items: [Item] = []) { self.items = items }
  }

  /// What a task is, which picks its glyph and what opening it shows (T37.29.8).
  public enum Kind: Equatable, Sendable {
    /// A subagent, whose transcript opens.
    case agent
    /// A background shell, whose output opens.
    case shell
  }

  /// A subagent or a background call.
  public struct Item: Equatable, Sendable {
    /// The core's id for it, which opening its transcript names.
    public var id: String
    /// `reviewer`, `bash: cargo nextest run`.
    public var label: String
    /// The model tier it runs on, `cheap`.
    public var tier: String
    public var kind: Kind
    public var state: ToolHeader.State
    /// What it cost, `$0.03`; `nil` while it runs.
    public var cost: String?

    public init(
      id: String, label: String, tier: String, kind: Kind, state: ToolHeader.State,
      cost: String? = nil
    ) {
      (self.id, self.label, self.tier, self.kind, self.state, self.cost) = (
        id, label, tier, kind, state, cost
      )
    }
  }

  /// What the tab asks the app to do.
  public enum Intent: Equatable, Sendable {
    /// Open the task with this id: a subagent's transcript or a shell's output.
    case open(task: String)
  }

  let state: State
  let send: @MainActor (Intent) -> Void

  public init(state: State, send: @escaping @MainActor (Intent) -> Void) {
    (self.state, self.send) = (state, send)
  }

  public var body: some View {
    if state.items.isEmpty {
      Text("No tasks yet")
        .textStyle(.caption)
        .foregroundStyle(Color(.textSecondary))
    } else {
      InspectorSection(state.title) {
        ForEach(state.items, id: \.id) { item in
          let open = Self.openAction(item.id, kind: item.kind, send: send)
          TaskItemRow(item: item, actions: [open])
            .onTapGesture { open.perform() }
            .accessibilityAction { open.perform() }
        }
      }
    }
  }
}

extension TasksTab.State {
  /// `Subagents & background · 3`.
  var title: String { "Subagents & background · \(items.count)" }
}

extension TasksTab {
  /// A row's action, which a click on the row performs too: open the task, labelled by what
  /// opening its kind shows.
  nonisolated static func openAction(
    _ task: String, kind: Kind, send: @escaping @MainActor (Intent) -> Void
  ) -> RowAction {
    RowAction(title: kind.openTitle, symbol: kind.openSymbol) { send(.open(task: task)) }
  }
}

extension TasksTab.Kind {
  /// The DS§3.7 glyph the transcript's tool row shows for it: `person.2` agent, `terminal` shell.
  var symbol: String {
    switch self {
    case .agent: "person.2"
    case .shell: "terminal"
    }
  }

  /// The open action's title, which is also its tooltip.
  var openTitle: String {
    switch self {
    case .agent: "Open transcript"
    case .shell: "Open output"
    }
  }

  /// The open action's DS§3.7 glyph: `text.bubble` for a transcript, `doc.text` for output.
  var openSymbol: String {
    switch self {
    case .agent: "text.bubble"
    case .shell: "doc.text"
    }
  }
}

/// An `InspectorRow` with the task kind's glyph (`person.2` or `terminal`, as the transcript's
/// tool row shows it): the label, the tier as a Badge, the cost in `text.secondary`, then the
/// ToolHeader's spinner, check or cross.
private struct TaskItemRow: View {
  let item: TasksTab.Item
  let actions: [RowAction]

  var body: some View {
    InspectorRow(symbol: item.kind.symbol, isSelected: false, actions: actions) {
      Text(item.label).frame(maxWidth: .infinity, alignment: .leading)
      Badge(item.tier)
      if let cost = item.cost {
        Text(cost)
          .textStyle(.footnote, tabularDigits: true)
          .foregroundStyle(Color(.textSecondary))
      }
      ToolHeaderStatus(state: item.state, duration: nil)
    }
  }
}

#Preview("tasks") { PreviewMatrix { TasksInspectorSample(state: PreviewState.tasks) } }
#Preview("empty") { PreviewMatrix { TasksInspectorSample(state: .init()) } }
