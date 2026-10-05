// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixture for dropped project values (T37.30.4): a project file that raises the
// session budget, the value listed as dropped with its reason above the budget table. Separate
// from `PreviewState+SettingsScreen.swift` so each card adds its fixtures without editing another's.

import SwiftUI

extension PreviewState {
  /// The Budget page after the guard list threw out the project's `session_usd = 999.0`.
  static let settingsBudgetDropped = SettingsScreenState(
    pages: SettingsPage.allCases, selection: .budget,
    tables: [
      .init(
        id: "budget",
        fields: [
          .init(
            id: "budget.session_usd", title: "Session usd",
            detail: "Session spend cap, in USD.", source: .default,
            control: .field("5.0"))
        ])
    ],
    userFile: userFile, projectFile: projectFile,
    dropped: [
      .init(
        id: "budget.session_usd", reason: "A project may not raise a budget above your own",
        change: "999 → 5")
    ])
}
