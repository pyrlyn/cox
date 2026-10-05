// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The General page's shortcuts box (T51.15, DT§4.6): one row per global shortcut, "Show cox
// menu" and "New session", with the control that records it. Composition only (DS§5): the rows
// arrive in the state, and the recorder is the app's slot, since the hotkey library that draws
// and stores it is linked by the app alone. Apart from `SettingsScreen.swift` so the box is its
// own card.

import SwiftUI

extension SettingsScreen {
  /// A shortcut the person records; none is bound until they do.
  public struct Shortcut: Identifiable, Equatable, Sendable {
    /// The hotkey's name, which the app's recorder stores it under.
    public let id: String
    var title: String
    var detail: String

    public init(id: String, title: String, detail: String) {
      (self.id, self.title, self.detail) = (id, title, detail)
    }
  }
}

/// A `SettingsGroupBox` with one `TitledSetting` per shortcut, its recorder at the trailing edge.
struct ShortcutsBox: View {
  let shortcuts: [SettingsScreen.Shortcut]
  let recorder: @MainActor (SettingsScreen.Shortcut.ID) -> AnyView

  var body: some View {
    SettingsGroupBox("Shortcuts") {
      ForEach(shortcuts) { shortcut in
        TitledSetting(
          title: shortcut.title, detail: shortcut.detail, control: recorder(shortcut.id)
        )
        // `SettingRow`'s insets; a shortcut is UI state, not config, so no layer badge.
        .padding(.horizontal, Space.l)
        .padding(.vertical, Space.ml)
      }
    }
  }
}

#Preview("shortcuts") {
  SettingsScreen(
    state: PreviewState.settingsGeneral, recorder: PreviewState.shortcutRecorder, send: { _ in }
  )
  .frame(width: Size.windowMinWidth, height: Size.windowMinHeight)
  .padding(Space.xxl)
  .background(PreviewBackdrop())
}
