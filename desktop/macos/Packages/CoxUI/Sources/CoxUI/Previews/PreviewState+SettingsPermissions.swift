// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the Permissions page's rules and grants (T37.45.3, mockup 19):
// the page's tables without the rule lists `SettingsStore` keeps out of them, the rules from
// three layers with the project's locked, a session grant, and the same page after Rust refused
// a rule. Separate from `PreviewState+SettingsScreen.swift` so each card adds its fixtures
// without editing another's.

import SwiftUI

extension PreviewState {
  /// Mockup 19's Permissions page with its rules and one session grant.
  static let settingsPermissionRules = permissionRules(failure: nil)

  /// The page after a rule with no closing bracket was sent.
  static let settingsPermissionRulesRefused = permissionRules(
    failure: "`Bash(git push` is not a rule: missing closing ')'")

  private static func permissionRules(failure: String?) -> SettingsScreenState {
    var state = settingsPermissions
    state.tables = state.tables.map { table in
      let fields = table.fields.filter { $0.id != "permissions.deny" }
      return SettingsScreen.Table(id: table.id, fields: fields, key: table.key)
    }
    state.permissions = .init(
      rules: [
        .init(kind: .deny, text: "Bash(rm -rf:*)", source: .default),
        .init(kind: .deny, text: "Read(~/.ssh/**)", source: .default),
        .init(kind: .allow, text: "Bash(cargo nextest:*)", source: .user),
        .init(kind: .allow, text: "Bash(git status)", source: .user),
        .init(kind: .ask, text: "Edit(**/*.lock)", source: .project),
      ],
      grants: [.init(id: "g1", subject: "bash git push", detail: "in “Add retry jitter…”")],
      failure: failure)
    return state
  }
}
