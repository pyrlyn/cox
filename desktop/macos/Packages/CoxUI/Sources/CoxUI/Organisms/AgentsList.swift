// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `AgentsList` (DT§3.3.1, mockup 27's Info › Agents, T52.8): who can drive a session in this
// project — cox, then each external ACP agent — with where each came from, the line cox launches
// it with, and why it cannot start here. Separate so the Info tab and the New-session sheet show
// one agent the same way, from values the core formatted.

import SwiftUI

/// An `InspectorSection` of agent rows over the line that says cox still sandboxes what it spawns.
public struct AgentsList: View {
  /// One agent as the core lists it.
  public struct Row: Equatable, Sendable, Identifiable {
    /// The config name; empty for cox itself.
    public var id: String
    /// What the UI calls it: "Claude Agent", never "Claude Code".
    public var label: String
    /// `built-in`, `user config` or `plugin <id>`.
    public var origin: String
    /// The program, args, key, network and writable directories; empty for cox.
    public var launch: String
    /// Why it cannot start here; `nil` when it can.
    public var unavailable: String?

    public init(
      id: String, label: String, origin: String, launch: String = "", unavailable: String? = nil
    ) {
      (self.id, self.label, self.origin, self.launch, self.unavailable) =
        (id, label, origin, launch, unavailable)
    }
  }

  let rows: [Row]

  public init(rows: [Row]) { self.rows = rows }

  public var body: some View {
    InspectorSection("Agents") {
      ForEach(rows) { AgentRow(row: $0).padding(.vertical, Space.xs) }
      Text(
        "External agents render in the same transcript and review UI. cox still applies its "
          + "own sandbox to the process it spawns."
      )
      .textStyle(.caption)
      .foregroundStyle(Color(.textSecondary))
      .fixedSize(horizontal: false, vertical: true)
      .padding(.top, Space.m)
    }
  }
}

/// The agent's name, its origin and launch line under it, and why it cannot start in
/// `status.warning` — the colour is on the words here because there is no symbol to carry it.
struct AgentRow: View {
  let row: AgentsList.Row

  private var detail: String {
    row.launch.isEmpty ? row.origin : "\(row.origin) · \(row.launch)"
  }

  var body: some View {
    HStack(alignment: .firstTextBaseline, spacing: Space.m) {
      Image(systemName: row.id.isEmpty ? "sparkle" : "powerplug")
        .symbolStyle(.body)
        .foregroundStyle(row.id.isEmpty ? Color(.roleProject) : Color(.textSecondary))
        .accessibilityHidden(true)
      VStack(alignment: .leading, spacing: Space.xxs) {
        Text(row.label).textStyle(.body).foregroundStyle(Color(.textPrimary))
        Text(detail)
          .textStyle(.caption)
          .foregroundStyle(Color(.textSecondary))
          .lineLimit(2)
          .truncationMode(.middle)
        if let why = row.unavailable {
          Text(why)
            .textStyle(.caption)
            .foregroundStyle(Color(.statusWarning))
            .fixedSize(horizontal: false, vertical: true)
        }
      }
      .frame(maxWidth: .infinity, alignment: .leading)
    }
    .accessibilityElement(children: .combine)
  }
}

#Preview("agents") { PreviewMatrix { AgentsListSample(rows: PreviewState.agents) } }
