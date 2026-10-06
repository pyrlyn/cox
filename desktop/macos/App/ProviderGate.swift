// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// What the session window does about a provider that cannot answer (DT§5.3, T60.5, A139): the
// composer's "Add key" opens Settings at Models & Providers, and the usable providers and each
// open session's readiness are read again when the window becomes key and when a provider key is
// stored in Settings. Separate from `SessionWindow` to keep its body the layout; the rule itself
// is the core's, read through `ComposerStore`.

import CoxClient
import CoxModel
import SwiftUI

extension SessionWindow {
  /// "Add key" under the composer: Settings at Models & Providers (T60.5).
  func openProviders() {
    model.settings?.requestedGroup = .models
    openSettings()
  }

  /// Reads which providers can answer, and each open session's readiness again: on the window
  /// becoming key and after a provider key is stored (DT§5.3, T60.5). The core re-probes on
  /// every call, so a key added in Settings counts at once.
  func refreshProviders() async {
    if let live = try? model.launch.live.get() {
      usable = try? await live.usableProviders(cwd: LaunchCore.project())
    }
    for session in opened.values { await session.composer.refreshReadiness() }
    await refreshModelSections()
  }

  /// The model popover's sections, asked again so a provider that gained or lost its key is
  /// greyed or listed at once (T60.7); the core marks each by the same `usable` list the footer
  /// reads. A remote session has no local config to read, so its sections stay empty.
  private func refreshModelSections() async {
    guard let live = try? model.launch.live.get() else { return }
    for (id, session) in opened where !session.modelSections.isEmpty {
      let cwd = model.sidebar.entry(id)?.session.cwd ?? LaunchCore.project()
      opened[id]?.modelSections = await live.modelSections(cwd: cwd, usable: usable)
    }
  }
}

/// Runs `refresh` when the window becomes the key window and when the provider keys Settings
/// holds change.
private struct ProviderRefresh: ViewModifier {
  let settings: SettingsStore?
  let refresh: () async -> Void
  @Environment(\.controlActiveState) private var activeState

  func body(content: Content) -> some View {
    content
      .onChange(of: settings?.storedKeys) { Task { await refresh() } }
      .onChange(of: activeState) { if activeState == .key { Task { await refresh() } } }
  }
}

extension View {
  func refreshingProviders(
    onKeysOf settings: SettingsStore?, refresh: @escaping () async -> Void
  ) -> some View {
    modifier(ProviderRefresh(settings: settings, refresh: refresh))
  }
}
