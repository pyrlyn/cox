// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `AgentPickerSheet` (DT§3.3.1, mockup 27, T52.8): New session asks who drives it — cox or an
// external ACP agent. An agent that cannot start here is listed with why, and cannot be picked.
// Separate so the app only presents it: CoxModel's `AgentPicker` owns the list and the rule, the
// sheet reports what the person picks as intents.

import SwiftUI

/// A caption, one row per agent with a checkmark on the picked one, the last failure in
/// `status.danger`, then Cancel and Start.
public struct AgentPickerSheet: View {
  public struct State: Equatable, Sendable {
    /// Cox first, then each external agent.
    public var rows: [AgentsList.Row]
    /// The picked row's id; empty is cox.
    public var selection: String
    /// Why the list could not be read; the sheet then offers cox alone.
    public var failure: String?

    public init(rows: [AgentsList.Row] = [], selection: String = "", failure: String? = nil) {
      (self.rows, self.selection, self.failure) = (rows, selection, failure)
    }
  }

  public enum Intent: Equatable, Sendable {
    case choose(String)
    case start
    case cancel
  }

  let state: State
  let send: (Intent) -> Void

  public init(state: State, send: @escaping (Intent) -> Void) {
    (self.state, self.send) = (state, send)
  }

  public var body: some View {
    VStack(alignment: .leading, spacing: Space.l) {
      Text("New Session").textStyle(.body).fontWeight(.semibold)
        .foregroundStyle(Color(.textPrimary))
        .accessibilityAddTraits(.isHeader)
      Text(
        "Who drives it. An external agent brings its own model, auth and billing; cox sandboxes "
          + "its process and asks you before it acts."
      )
      .textStyle(.caption)
      .foregroundStyle(Color(.textSecondary))
      .fixedSize(horizontal: false, vertical: true)
      VStack(alignment: .leading, spacing: Space.xs) {
        ForEach(state.rows) { row in
          Button {
            send(.choose(row.id))
          } label: {
            HStack(alignment: .firstTextBaseline, spacing: Space.m) {
              AgentRow(row: row)
              Image(systemName: "checkmark")
                .symbolStyle(.body)
                .foregroundStyle(Color(.accent))
                .opacity(row.id == state.selection ? 1 : 0)
                .accessibilityHidden(true)
            }
            .padding(.horizontal, Space.ml)
            .padding(.vertical, Space.s)
            .contentShape(Rectangle())
          }
          .buttonStyle(.plain)
          .disabled(row.unavailable != nil)
          .accessibilityAddTraits(row.id == state.selection ? .isSelected : [])
        }
      }
      .insetWell(Color(.fillPrimary), cornerRadius: Radius.m)
      if let failure = state.failure {
        Text(failure).textStyle(.caption).foregroundStyle(Color(.statusDanger))
          .fixedSize(horizontal: false, vertical: true)
      }
      HStack(spacing: Space.s) {
        Spacer(minLength: 0)
        Button("Cancel") { send(.cancel) }
          .buttonStyle(CoxButtonStyle(.secondary, size: .small))
          .keyboardShortcut(.cancelAction)
        Button("Start") { send(.start) }
          .buttonStyle(CoxButtonStyle(.primary, size: .small))
          .keyboardShortcut(.defaultAction)
      }
    }
    .padding(Space.xl)
    .frame(width: Size.popoverWidth)
  }
}

#Preview("agents") {
  PreviewMatrix { AgentPickerSheet(state: PreviewState.agentPicker) { _ in } }
}
