// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The app's shortcuts (T51.17): "Ask cox in <project>" and "Open <session> in cox" for Siri,
// Spotlight and the Shortcuts app, and the one place the intents get the launch's `AppModel`.

import AppIntents

struct CoxShortcuts: AppShortcutsProvider {
  static var appShortcuts: [AppShortcut] {
    AppShortcut(
      intent: AskCoxIntent(), phrases: ["Ask \(.applicationName) in \(\.$project)"],
      shortTitle: "Ask cox", systemImageName: "text.bubble")
    AppShortcut(
      intent: OpenSessionIntent(), phrases: ["Open \(\.$session) in \(.applicationName)"],
      shortTitle: "Open session", systemImageName: "macwindow")
  }
}

@MainActor
enum CoxIntents {
  /// Once per launch: the intents' `@Dependency` resolves to `model`, and the phrases pick up
  /// the projects and sessions there are now.
  static func register(_ model: AppModel) {
    AppDependencyManager.shared.add(dependency: model)
    CoxShortcuts.updateAppShortcutParameters()
  }
}
