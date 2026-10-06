// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ComposerChipRow` (DS§6.4 row `Composer`; DT§5.3): the row under the composer's editor — the
// paperclip, the permission mode's menu, the model chip that opens the model popover, think,
// shell mode, mentions, the queue, the token meter and Send. Separate from `Composer` only to
// keep that file within SwiftLint's length limit; it reports through the composer's intents.

import SwiftUI

/// The paperclip, the mode menu, the model chip and think, shell mode and its switch, mentions, the queue, Send.
struct ComposerChipRow: View {
  let state: Composer.State
  let send: (Composer.Intent) -> Void

  var body: some View {
    HStack(spacing: Space.s) {
      Button {
        send(.attach)
      } label: {
        ComposerChip("", kind: .attachment)
      }
      .buttonStyle(.plain)
      .help("Attach files")
      .accessibilityLabel("Attach files")
      if let mode = state.mode {
        ModeMenu(mode: mode) { send(.setMode($0)) }
      }
      if let model = state.model {
        Button {
          send(.openModel)
        } label: {
          ComposerChip(model, kind: .model)
        }
        .buttonStyle(.plain)
        // The screen hangs the model popover over this chip.
        .anchorPreference(key: ModelChipAnchor.self, value: .bounds) { $0 }
        .help("Model")
        .accessibilityLabel("Model")
        .accessibilityValue(model)
        ThinkChip(isOn: state.think) { send(.toggleThink) }
      }
      if state.isShell {
        ComposerChip("Shell", kind: .shell) { send(.leaveShell) }
        Toggle(
          "Share output", isOn: Binding(get: { state.shareOutput }, set: { send(.shareOutput($0)) })
        )
        .toggleStyle(CoxToggleStyle())
      }
      ForEach(state.mentions) { mention in
        ComposerChip(mention.label, kind: .mention) { send(.removeMention(mention.id)) }
      }
      if state.queued > 0 {
        ComposerChip("Queued · \(state.queued)", kind: .queued)
      }
      Spacer(minLength: Space.m)
      if let meter = state.meter {
        // Its figures never wrap; the chips before it truncate instead.
        TokenMeter(state: meter, isOpen: state.tokens != nil) { send(.toggleTokens) }.fixedSize()
      }
      Button {
        send(.submit)
      } label: {
        Image(systemName: "arrow.up").symbolStyle(.body)
      }
      .buttonStyle(SendButtonStyle())
      .disabled(!state.canSend)
      .help(state.isRunning ? "Queue after this turn (⏎) · send now (⌘⏎)" : "Send (⏎)")
      .accessibilityLabel(state.isRunning ? "Queue" : "Send")
    }
  }
}
