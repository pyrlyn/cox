// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for mockup 18's provider keys and pop-ups (T37.45.2): one provider
// whose key the Keychain holds and one with none, and the code tier's model and effort as
// pop-ups beside a thinking switch that stays segmented. Separate so this card's fixtures stay
// out of `PreviewState+SettingsScreen.swift`, whose `field` helper they share.

import SwiftUI

extension PreviewState {
  static let popUpOptions = ["low", "medium", "high", "xhigh"]

  /// Mockup 18's Models & Providers page: a stored key, a missing one, and the tier's pop-ups.
  static let settingsProviders = SettingsScreenState(
    pages: SettingsPage.allCases, selection: .models,
    tables: [
      .init(
        id: "providers.anthropic",
        fields: [field("providers.anthropic.base_url", .default, .field(fieldText))],
        key: .init(provider: "anthropic", isStored: true)),
      .init(
        id: "providers.openrouter",
        fields: [
          field("providers.openrouter.base_url", .user, .field("https://openrouter.ai/api/v1"))
        ],
        key: .init(provider: "openrouter", isStored: false)),
      .init(
        id: "tiers.code",
        fields: [
          field(
            "tiers.code.effort", .user,
            .menu("high", options: popUpOptions.map { .init(value: $0, title: $0) })),
          field(
            "tiers.code.model", .user,
            .menu(
              "claude-sonnet-5",
              options: [
                .init(value: "claude-sonnet-5", title: "Sonnet 5"),
                .init(value: "claude-opus-5-5", title: "Opus 5.5"),
              ])),
          field("tiers.code.thinking", .default, .choice("off", options: ["off", "adaptive"])),
        ]),
    ],
    userFile: userFile, projectFile: projectFile)
}

/// The key row in both states, for previews and snapshots.
enum KeyRowSample {
  static var stored: some View {
    KeyRow(key: .init(provider: "anthropic", isStored: true)) { _ in }
  }
  static var none: some View {
    KeyRow(key: .init(provider: "openrouter", isStored: false)) { _ in }
  }
}
