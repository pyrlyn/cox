// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The dropped-values box (T37.30.4, DT§5.7): the values the project's `.cox/config.toml` set that
// the guard list threw out, each with its reason and what holds instead. Composition only (DS§5):
// the key, the reason and the change arrive in the state from `CoxModel`'s
// `SettingsStore.dropped(in:)`. Apart from `SettingsScreen.swift` so the box is its own card.

import SwiftUI

extension SettingsScreen {
  public struct DroppedValue: Identifiable, Equatable, Sendable {
    /// The dotted config key.
    public let id: String
    /// Why the project may not set it.
    var reason: String
    /// `999 → 5`: what the project set, then what holds.
    var change: String

    public init(id: String, reason: String, change: String) {
      (self.id, self.reason, self.change) = (id, reason, change)
    }
  }
}

/// A `SettingsGroupBox` with one `TitledSetting` per dropped value, its change in a warning badge.
struct DroppedBox: View {
  let values: [SettingsScreen.DroppedValue]

  var body: some View {
    SettingsGroupBox("Dropped from the project") {
      ForEach(values) { value in
        TitledSetting(
          title: value.id, detail: value.reason, control: Badge(value.change, kind: .warning)
        )
        // `SettingRow`'s insets; the value is in no layer, so no source badge.
        .padding(.horizontal, Space.l)
        .padding(.vertical, Space.ml)
      }
    }
  }
}

#Preview("dropped") {
  SettingsScreen(state: PreviewState.settingsBudgetDropped) { _ in }
    .frame(width: Size.windowMinWidth, height: Size.windowMinHeight)
    .padding(Space.xxl)
    .background(PreviewBackdrop())
}
