// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the MCP page's logins (T37.30.3): an HTTP server before and after
// its login, and a stdio server with none, over the page's `mcp` table. Separate from
// `PreviewState+SettingsScreen.swift` so each card adds its fixtures without editing another's.

import SwiftUI

extension PreviewState {
  /// The MCP page with `docs` logged out.
  static let settingsMcp = settingsMcp(
    docs: .init(id: "docs", detail: "Not logged in", action: .logIn))

  /// The same page after the browser came back.
  static let settingsMcpLoggedIn = settingsMcp(
    docs: .init(id: "docs", detail: "Logged in, expires in 1h", action: .logOut))

  private static func settingsMcp(docs: SettingsScreen.Login) -> SettingsScreenState {
    SettingsScreenState(
      pages: SettingsPage.allCases, selection: .mcp,
      tables: [
        .init(
          id: "mcp",
          fields: [
            .init(
              id: "mcp.timeout_s", title: "Timeout s", detail: "Per-call timeout, in seconds.",
              source: .default, control: .field("30"))
          ])
      ],
      userFile: userFile,
      logins: [
        docs,
        .init(id: "local", detail: "Runs locally from .mcp.json; no login", action: nil),
      ])
  }
}
