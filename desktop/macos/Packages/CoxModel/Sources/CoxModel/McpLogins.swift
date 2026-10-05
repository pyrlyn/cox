// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The MCP page's logins (T37.30.3, DT§5.7): per server, the line saying whether cox can reach
// it and the button that changes that, both worded by `cox_app` (T58.4.1), with its status badge
// and log (T37.45.4). Here, not in CoxUI, because the store owns what the screen shows (DS§1);
// the app copies them into CoxUI's `SettingsScreen.Login` field for field.

import CoxClient

public struct McpLoginRow: Identifiable, Equatable, Sendable {
  public typealias Action = McpLoginAction

  public let server: String
  public let detail: String
  /// `nil` for a server with no login.
  public let action: Action?
  /// The badge (T37.45.4).
  public var status: McpStatus = .unknown
  /// What Show log opens; no button when empty.
  public var log: [String] = []
  public var id: String { server }
}

extension SettingsStore {
  /// One row per MCP server in effect, in name order.
  public var logins: [McpLoginRow] {
    (view?.mcp ?? []).map { server in
      McpLoginRow(
        server: server.name, detail: server.detail, action: server.action, status: server.status,
        log: server.log)
    }
  }
}
