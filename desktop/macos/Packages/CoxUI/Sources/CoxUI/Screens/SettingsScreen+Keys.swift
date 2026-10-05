// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A provider's key row and its sheet (T37.45.2, DT§5.7, mockup 18's Add key / Change key): the row
// says whether the Keychain holds a key — never the key — and its button opens a sheet with a
// secure field. What is typed stays in the sheet until Save hands it to `.storeKey`, which the app
// sends to `SecretStore` alone; the sheet forgets it on Save or Cancel. Apart from
// `SettingsScreen.swift` so the key's path is one file to audit.

import SwiftUI

/// `API key` with whether one is stored, then Add key (primary) or Change key (secondary).
struct KeyRow: View {
  let key: SettingsScreen.Key
  let send: (SettingsScreenIntent) -> Void
  @State private var isEntering = false

  var body: some View {
    TitledSetting(
      title: "API key", detail: key.isStored ? "Key in Keychain" : "No key",
      control: Button(key.isStored ? "Change key" : "Add key") { isEntering = true }
        .buttonStyle(CoxButtonStyle(key.isStored ? .secondary : .primary, size: .small))
    )
    // `SettingRow`'s insets; a key has no config layer, so no badge.
    .padding(.horizontal, Space.l)
    .padding(.vertical, Space.ml)
    .sheet(isPresented: $isEntering) {
      KeySheet(provider: key.provider, isStored: key.isStored) { secret in
        if let secret { send(.storeKey(provider: key.provider, secret: secret)) }
        isEntering = false
      }
    }
  }
}

/// The provider's name, where the key goes, a secure field, then Cancel and Save.
struct KeySheet: View {
  let provider: String
  let isStored: Bool
  /// The typed key on Save, `nil` on Cancel.
  let finish: (String?) -> Void
  @State private var secret = ""

  var body: some View {
    let isBlank = secret.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    VStack(alignment: .leading, spacing: Space.l) {
      SettingLabel(
        "\(provider) API key", detail: "Kept in the Keychain as cox/\(provider), never in a file",
        namesItem: true)
      SecureField(
        "API key", text: $secret,
        prompt: Text(isStored ? "New key" : "Paste key").foregroundStyle(Color(.textTertiary))
      )
      .settingWell(width: nil)
      HStack(spacing: Space.m) {
        Spacer()
        Button("Cancel") { finish(nil) }
          .buttonStyle(CoxButtonStyle(.secondary, size: .small))
          .keyboardShortcut(.cancelAction)
        Button("Save", action: save)
          .buttonStyle(CoxButtonStyle(.primary, size: .small))
          .keyboardShortcut(.defaultAction)
          .disabled(isBlank)
      }
    }
    .padding(Space.xl)
    .frame(width: Size.popoverWidth)
  }

  /// Return reaches Save as the default action, so one path sends the key.
  private func save() {
    let typed = secret
    secret = ""
    if !typed.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { finish(typed) }
  }
}

#Preview("no key") { PreviewMatrix { KeyRowSample.none } }
#Preview("stored") { PreviewMatrix { KeyRowSample.stored } }
#Preview("sheet") { PreviewMatrix { KeySheet(provider: "anthropic", isStored: true) { _ in } } }
