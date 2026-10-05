// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the MCP page's status badges and log (T37.45.4): mockup 20's
// servers — two connected, one needing a login, one failed with a log — plus one no session has
// tried yet, and the same page with MCP off. Separate from `PreviewState+SettingsLogins.swift`
// so each card adds its fixtures without editing another's.

import SwiftUI

extension PreviewState {
  /// Why `sentry` failed, as Rust hands it over: sanitized, one line per entry.
  static let mcpLog = [
    "skipped: handshake: connection closed: initialize response",
    "uvx: command not found: sentry-mcp",
  ]

  /// Mockup 20's servers, and `notes`, added since the last session.
  static let settingsMcpStatuses = settingsMcpPage([
    .init(
      id: "github", detail: "Logged in, expires in 3h", action: .logOut, status: .connected),
    .init(id: "linear", detail: "Not logged in", action: .logIn, status: .needsLogin),
    .init(id: "notes", detail: "Runs locally from .mcp.json; no login", action: nil),
    .init(
      id: "postgres", detail: "Runs locally from config; no login", action: nil,
      status: .connected),
    .init(
      id: "sentry", detail: "Runs locally from .mcp.json; no login", action: nil,
      status: .failed, log: mcpLog),
  ])

  /// MCP turned off: every server says so.
  static let settingsMcpDisabled = settingsMcpPage([
    .init(id: "github", detail: "Logged in", action: .logOut, status: .disabled),
    .init(
      id: "postgres", detail: "Runs locally from config; no login", action: nil,
      status: .disabled),
  ])

  private static func settingsMcpPage(_ logins: [SettingsScreen.Login]) -> SettingsScreenState {
    SettingsScreenState(
      pages: SettingsPage.allCases, selection: .mcp, userFile: userFile, logins: logins)
  }
}
