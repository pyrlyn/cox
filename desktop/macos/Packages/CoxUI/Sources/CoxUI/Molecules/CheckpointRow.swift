// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `CheckpointRow` (DS§6.3 row `CheckpointRow`, the inspector's Checkpoints `.fr`): one point
// the session can be rewound to — which turn, before what — and when it was taken. Separate so
// the inspector and the rewind list draw a checkpoint the same way.

import SwiftUI

/// An `InspectorRow`: a clock, the checkpoint's label, then its time in `text.secondary` —
/// not the mockup's tertiary, so it stays readable on frosted glass (DS§8).
public struct CheckpointRow: View {
  /// A checkpoint, formatted by the core.
  public struct Checkpoint: Equatable, Sendable {
    /// The core's id for it, which a rewind names.
    public var id: String
    /// `Turn 1 · before edit retry.rs`.
    public var label: String
    /// When it was taken, `14:02`.
    public var time: String

    public init(id: String, label: String, time: String) {
      (self.id, self.label, self.time) = (id, label, time)
    }
  }

  let checkpoint: Checkpoint
  let isSelected: Bool
  let actions: [RowAction]

  init(_ checkpoint: Checkpoint, isSelected: Bool = false, actions: [RowAction] = []) {
    self.checkpoint = checkpoint
    self.isSelected = isSelected
    self.actions = actions
  }

  public var body: some View {
    InspectorRow(symbol: "clock", isSelected: isSelected, actions: actions) {
      Text(checkpoint.label).frame(maxWidth: .infinity, alignment: .leading)
      Text(checkpoint.time)
        .textStyle(.footnote, tabularDigits: true)
        .foregroundStyle(Color(.textSecondary))
    }
  }
}

#Preview("selected") { PreviewMatrix { CheckpointSample(isSelected: true) } }
#Preview("list") { PreviewMatrix { CheckpointList() } }
