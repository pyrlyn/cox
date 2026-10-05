// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PlanTab` (DS§6.4 row `PlanTab`, the mockup's Plan `.ib` and `.todo`; DT§5.1 Plan): the
// inspector's second tab — the live todo list the agent keeps with its `todo` tool, each step with
// its state. Separate so the `Inspector` frame stays a slot and each tab is its own view, fed plain
// values the app copies from the core (T37.29.2).

import SwiftUI

/// One `InspectorSection` of the plan's steps in the model's order; with none, one quiet line.
/// Read-only: the list is the model's, so the tab sends nothing.
public struct PlanTab: View {
  /// What the tab lists, as the latest `todo` call left it.
  public struct State: Equatable, Sendable {
    public var items: [Item] = []

    public init(items: [Item] = []) { self.items = items }
  }

  /// One step of the plan.
  public struct Item: Equatable, Sendable {
    /// Unique within the list.
    public var id: String
    public var text: String
    public var step: Step

    public init(id: String, text: String, step: Step) {
      (self.id, self.text, self.step) = (id, text, step)
    }
  }

  /// Where a step stands.
  public enum Step: Equatable, Sendable {
    case pending, inProgress, done
  }

  let state: State

  public init(state: State) { self.state = state }

  public var body: some View {
    if state.items.isEmpty {
      Text("No plan yet")
        .textStyle(.caption)
        .foregroundStyle(Color(.textSecondary))
    } else {
      InspectorSection(state.title) {
        ForEach(state.items, id: \.id) { PlanStepRow(item: $0) }
      }
    }
  }
}

extension PlanTab.State {
  /// `Plan · 2 of 5`: the steps done of all.
  var title: String { "Plan · \(items.count { $0.step == .done }) of \(items.count)" }
}

extension PlanTab.Step {
  /// The mockup's `.box`: empty, filled in `accent` while worked on, checked in green once done.
  var symbol: String {
    switch self {
    case .pending: "square"
    case .inProgress: "square.inset.filled"
    case .done: "checkmark.square.fill"
    }
  }

  var colour: Color {
    switch self {
    case .pending: Color(.textSecondary)
    case .inProgress: Color(.accent)
    case .done: Color(.statusSuccess)
    }
  }

  /// What VoiceOver reads after the step's text.
  var label: String {
    switch self {
    case .pending: "Pending"
    case .inProgress: "In progress"
    case .done: "Done"
    }
  }
}

/// The step's box, then its text, wrapping; a done step is struck through in `text.secondary`.
private struct PlanStepRow: View {
  let item: PlanTab.Item

  var body: some View {
    let isDone = item.step == .done
    HStack(alignment: .firstTextBaseline, spacing: Space.m) {
      box.accessibilityHidden(true)
      Text(item.text)
        .strikethrough(isDone)
        .foregroundStyle(Color(isDone ? .textSecondary : .textPrimary))
        .frame(maxWidth: .infinity, alignment: .leading)
        .fixedSize(horizontal: false, vertical: true)
    }
    .textStyle(.compact)
    // The mockup's `.todo` sits the box almost on the section's edge, under its header.
    .padding(.horizontal, Space.xxs)
    .padding(.vertical, Space.s)
    .accessibilityElement(children: .combine)
    .accessibilityValue(item.step.label)
  }

  /// A done step's box is solid green with the check knocked out in `text.onAccent`, as the
  /// mockup's `.box.done`; the image's own palette mode wins over `symbolStyle`'s hierarchical
  /// one. The other boxes take their colour whole.
  @ViewBuilder private var box: some View {
    if item.step == .done {
      Image(systemName: item.step.symbol)
        .symbolRenderingMode(.palette)
        .symbolStyle(.body)
        .foregroundStyle(Color(.textOnAccent), item.step.colour)
    } else {
      Image(systemName: item.step.symbol).symbolStyle(.body).foregroundStyle(item.step.colour)
    }
  }
}

#Preview("plan") { PreviewMatrix { PlanInspectorSample(state: PreviewState.plan) } }
#Preview("empty") { PreviewMatrix { PlanInspectorSample(state: .init()) } }
