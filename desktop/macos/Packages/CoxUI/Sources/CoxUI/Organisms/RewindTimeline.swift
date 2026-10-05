// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `RewindTimeline` (DS§6.4 row `RewindTimeline`; DT§3 Rewind timeline, DT§5.4): the session's
// checkpoints oldest first, each rewinding code, the conversation or both to before its turn.
// Separate from `ChangesTab`, whose checkpoint rows only name a rewind, because this is where the
// scope is picked — for Review's turn list and a gutter mark's popover alike.

import SwiftUI

/// One `InspectorSection` of `CheckpointRow`s; a row's actions are the three scopes. The selected
/// row (the one a gutter mark points at) is lifted and shows them without a hover.
public struct RewindTimeline: View {
  /// What the timeline lists, formatted by the core.
  public struct State: Equatable, Sendable {
    /// Oldest first.
    public var checkpoints: [CheckpointRow.Checkpoint] = []
    /// The id of the lifted checkpoint.
    public var selection: String?

    public init(checkpoints: [CheckpointRow.Checkpoint] = [], selection: String? = nil) {
      (self.checkpoints, self.selection) = (checkpoints, selection)
    }
  }

  /// What the timeline asks the app to do.
  public enum Intent: Equatable, Sendable {
    /// Rewind to before the checkpoint's turn: its files, its conversation, or both.
    case rewind(checkpoint: String, code: Bool, conversation: Bool)
  }

  let state: State
  let send: @MainActor (Intent) -> Void

  public var body: some View {
    InspectorSection("Rewind") {
      ForEach(state.checkpoints, id: \.id) {
        CheckpointRow(
          $0, isSelected: $0.id == state.selection, actions: Self.actions($0.id, send: send))
      }
      if state.checkpoints.isEmpty {
        Text("No checkpoints yet")
          .textStyle(.caption)
          .foregroundStyle(Color(.textSecondary))
      }
    }
  }
}

extension RewindTimeline {
  /// A checkpoint's scopes (DT§3): its code, its conversation, both. The symbols are DS§3.7's
  /// doc, comment and rewind.
  nonisolated static func actions(
    _ id: String, send: @escaping @MainActor (Intent) -> Void
  ) -> [RowAction] {
    let scopes = [
      ("Restore code", "doc.text", true, false),
      ("Restore conversation", "text.bubble", false, true),
      ("Restore code and conversation", "arrow.uturn.backward", true, true),
    ]
    return scopes.map { title, symbol, code, conversation in
      RowAction(title: title, symbol: symbol) {
        send(.rewind(checkpoint: id, code: code, conversation: conversation))
      }
    }
  }
}

#Preview("timeline") { PreviewMatrix { RewindTimelineSample(state: PreviewState.rewind) } }
#Preview("empty") { PreviewMatrix { RewindTimelineSample(state: .init()) } }
