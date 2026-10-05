// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// New session's agent sheet (DT§3.3.1, mockup 27, T52.8): when the config names an external ACP
// agent, New session asks who drives the session; with cox alone, or on a recording, it opens at
// once. Wiring only — CoxModel's `AgentPicker` owns the list and the rule that an agent that
// cannot start is never picked. Separate from `SessionWindow` so the window's body stays the
// layout.

import CoxModel
import CoxUI
import SwiftUI

@MainActor
enum NewSession {
  /// The picker for a new session in the launch's project, or `nil` when there is nothing to
  /// ask: a recording, no live core, or cox the only choice with the list read cleanly.
  static func picker(_ launch: LaunchCore) async -> AgentPicker? {
    guard !launch.isFixture, let live = try? launch.live.get() else { return nil }
    let picker = AgentPicker(client: live)
    await picker.load(cwd: LaunchCore.project())
    return picker.choices.count > 1 || picker.failure != nil ? picker : nil
  }
}

extension View {
  /// Shows the sheet while `picking` is set; Start clears it and opens with the agent picked.
  func newSessionSheet(
    _ picking: Binding<AgentPicker?>, open: @escaping @MainActor (String?) async -> Void
  ) -> some View {
    sheet(
      isPresented: Binding(
        get: { picking.wrappedValue != nil }, set: { if !$0 { picking.wrappedValue = nil } })
    ) {
      if let picker = picking.wrappedValue {
        AgentPickerSheet(state: ShellState.picker(picker)) { intent in
          switch intent {
          case .choose(let id): picker.choose(id.isEmpty ? nil : id)
          case .cancel: picking.wrappedValue = nil
          case .start:
            picking.wrappedValue = nil
            Task { await open(picker.chosen) }
          }
        }
      }
    }
  }
}
