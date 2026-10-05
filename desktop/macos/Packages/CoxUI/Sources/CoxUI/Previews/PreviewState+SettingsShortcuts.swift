// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the General page's shortcuts (T51.15): both shortcuts unbound,
// with a stand-in for the app's recorder, which CoxUI does not link. Separate from
// `PreviewState+SettingsScreen.swift` so each card adds its fixtures without editing another's.

import SwiftUI

extension PreviewState {
  /// The General page with its two shortcuts, neither recorded yet.
  static let settingsGeneral = SettingsScreenState(
    pages: SettingsPage.allCases, selection: .general, userFile: userFile,
    shortcuts: [
      .init(
        id: "showCoxMenu", title: "Show cox menu",
        detail: "Opens the menu-bar panel from any app."),
      .init(
        id: "newSession", title: "New session", detail: "Starts a session in a new window."),
    ])

  /// The recorder's empty look, as the app's library draws it before a shortcut is set.
  @MainActor static func shortcutRecorder(_ id: SettingsScreen.Shortcut.ID) -> AnyView {
    AnyView(
      Text("Record Shortcut")
        .textStyle(.caption)
        .foregroundStyle(Color(.textSecondary))
        .padding(.horizontal, Space.m)
        .frame(height: Size.buttonHeight)
        .insetWell(Color(.fillPrimary), cornerRadius: Radius.m))
  }
}
