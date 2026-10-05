// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The MCP page's logins box (T37.30.3, DT§5.7): per MCP server, whether cox can reach it, its
// status badge (T37.45.4, mockup 20's `.badge`), Show log when something went wrong, and a Log in
// or Log out button. Composition only (DS§5): the line, badge, log and action arrive in the state
// from `CoxModel`'s `SettingsStore.logins`; the button reports `.setLogin`, and Rust opens the
// login page through the host. The log opens in `TaskOutputSheet`, the same terminal well a
// task's output uses; Rust sanitized and capped the lines. Apart from `SettingsScreen.swift` so
// the box is its own card.

import SwiftUI

/// What a server's login button does.
public enum LoginAction: Equatable, Sendable { case logIn, logOut }

/// A server's badge (T37.45.4).
public enum ServerStatus: CaseIterable, Equatable, Sendable {
  case connected, needsLogin, failed, disabled, unknown

  var label: String {
    switch self {
    case .connected: "connected"
    case .needsLogin: "needs login"
    case .failed: "failed"
    case .disabled: "disabled"
    case .unknown: "unknown"
    }
  }

  /// Mockup 20's `b-env`, `b-warn` and `b-red`; the rest neutral.
  var kind: Badge.Kind {
    switch self {
    case .connected: .env
    case .needsLogin: .warning
    case .failed: .danger
    case .disabled, .unknown: .neutral
    }
  }
}

extension SettingsScreen {
  public struct Login: Identifiable, Equatable, Sendable {
    /// The server's name.
    public let id: String
    /// `Logged in, expires in 3h`, `Not logged in`, …
    var detail: String
    /// `nil` for a server with no login (stdio).
    var action: LoginAction?
    var status: ServerStatus
    /// What Show log opens; no button when empty.
    var log: [String]

    public init(
      id: String, detail: String, action: LoginAction?, status: ServerStatus = .unknown,
      log: [String] = []
    ) {
      (self.id, self.detail, self.action, self.status, self.log) = (
        id, detail, action, status, log
      )
    }
  }
}

/// A `SettingsGroupBox` with one `TitledSetting` per server, its button at the trailing edge.
struct LoginsBox: View {
  let logins: [SettingsScreen.Login]
  let send: (SettingsScreenIntent) -> Void
  /// The server whose log is open.
  @State private var shown: SettingsScreen.Login?

  var body: some View {
    SettingsGroupBox("Logins") {
      ForEach(logins) { login in
        TitledSetting(
          title: login.id, detail: login.detail, namesItem: true, control: controls(login)
        )
        // `SettingRow`'s insets; a login has no config layer, so no layer badge.
        .padding(.horizontal, Space.l)
        .padding(.vertical, Space.ml)
      }
    }
    .sheet(item: $shown) { login in
      TaskOutputSheet(title: "\(login.id) log", output: login.log.joined(separator: "\n")) {
        shown = nil
      }
    }
  }

  private func controls(_ login: SettingsScreen.Login) -> some View {
    HStack(spacing: Space.m) {
      Badge(login.status.label, kind: login.status.kind)
      if !login.log.isEmpty {
        Button("Show log") { shown = login }
          .buttonStyle(CoxButtonStyle(.secondary, size: .small))
      }
      button(login)
    }
  }

  @ViewBuilder private func button(_ login: SettingsScreen.Login) -> some View {
    if let action = login.action {
      Button(action == .logIn ? "Log in" : "Log out") {
        send(.setLogin(server: login.id, action == .logIn))
      }
      .buttonStyle(CoxButtonStyle(action == .logIn ? .primary : .secondary, size: .small))
    }
  }
}

#Preview("logins") {
  SettingsScreen(state: PreviewState.settingsMcp) { _ in }
    .frame(width: Size.windowMinWidth, height: Size.windowMinHeight)
    .padding(Space.xxl)
    .background(PreviewBackdrop())
}

#Preview("statuses") {
  SettingsScreen(state: PreviewState.settingsMcpStatuses) { _ in }
    .frame(width: Size.windowMinWidth, height: Size.windowMinHeight)
    .padding(Space.xxl)
    .background(PreviewBackdrop())
}

#Preview("log") {
  PreviewMatrix {
    TaskOutputSheet(title: "sentry log", output: PreviewState.mcpLog.joined(separator: "\n")) {}
  }
}
