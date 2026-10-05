// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// MCP logins on the Settings screen (T37.30.3, DT§5.7): each server in effect for a directory
// with whether cox holds a token for it, field for field as cox-ffi exports `cox_app::McpServer`.
// Separate from `Settings.swift` because a login is no config value: it lives in the token store
// and changes through Log in / Log out, whose page Rust hands to `PlatformHost.open`. The row's
// line and button come worded from `cox_app` (T58.4.1).

/// Whether cox can reach a server without asking the person to log in.
public enum McpLogin: Equatable, Sendable {
  /// A stdio server: it runs locally and has no login.
  case stdio
  case loggedOut
  /// `expires` is how long the token has left (`3h`), `nil` when the server gave no expiry.
  case loggedIn(expires: String?)
  /// Expired with no refresh token: log in again.
  case expired
  /// The token store could not be read (a locked keychain).
  case unreadable(error: String)
}

/// A server's badge (T37.45.4), as `cox_app::McpStatus` decides it from the config, the token
/// store and the last session opened in the project.
public enum McpStatus: Equatable, Sendable {
  case connected, needsLogin, failed, disabled
  /// No session in the project has tried the server yet.
  case unknown
}

/// The button a login row offers; `cox_app::LoginAction`.
public enum McpLoginAction: Equatable, Sendable { case logIn, logOut }

public struct McpServer: Identifiable, Equatable, Sendable {
  public var name: String
  /// Where it is configured: `config`, `.mcp.json`, `~/.claude.json`.
  public var source: String
  public var login: McpLogin
  public var status: McpStatus
  /// Why it failed, already sanitized and capped by Rust; empty when nothing went wrong.
  public var log: [String]
  /// The login's line, `Logged in, expires in 3h`.
  public var detail: String
  /// The button the row offers; `nil` for a server with no login.
  public var action: McpLoginAction?

  public var id: String { name }

  public init(
    name: String, source: String, login: McpLogin, detail: String, action: McpLoginAction?,
    status: McpStatus = .unknown, log: [String] = []
  ) {
    (self.name, self.source, self.login, self.status, self.log) = (
      name, source, login, status, log
    )
    (self.detail, self.action) = (detail, action)
  }

  /// The fixture client's login or logout: the state and words the core would send after it.
  mutating func fixtureLogin(_ login: Bool) {
    (self.login, detail, action) =
      login
      ? (.loggedIn(expires: "1h"), "Logged in, expires in 1h", .logOut)
      : (.loggedOut, "Not logged in", .logIn)
  }
}
