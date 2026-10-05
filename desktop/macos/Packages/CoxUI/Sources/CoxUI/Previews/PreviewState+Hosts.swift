// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for remote hosts (T52.21): a local project, then a connected host and a
// disconnected one, each a sidebar group under its host badge; and the Connect to Host sheet after
// a failed connect. Separate from `PreviewState+Window.swift` so this card adds its fixtures
// without editing the shell's.

import SwiftUI

extension PreviewState {
  static let hostSidebar = Sidebar.State(
    groups: [
      .init(
        id: "cox", title: "cox", kind: .project(isExpanded: true),
        sessions: [hostRow("jitter", .idle, "Add retry jitter", "2h ago · done", cost: "$0.42")]),
      .init(
        id: "host:devbox", title: "devbox", kind: .host(isConnected: true),
        sessions: [
          hostRow("deploy", .idle, "Fix the deploy script", "api", cost: "$0.18"),
          hostRow("logs", .idle, "Rotate the nginx logs", "ops"),
        ]),
      .init(
        id: "host:gpu-box", title: "gpu-box", kind: .host(isConnected: false),
        sessions: [
          hostRow("train", .idle, "Resume the eval run", "evals", isReadOnly: true)
        ]),
    ],
    selection: "deploy", providers: "3 providers", providerStatus: .running)

  static let connectHostFailed = ConnectHostSheet.State(
    failure: "ssh: Could not resolve hostname devbox: nodename nor servname provided")

  private static func hostRow(
    _ id: String, _ status: StatusDot.Status, _ title: String, _ subtitle: String,
    cost: String? = nil, isReadOnly: Bool = false
  ) -> Sidebar.Session {
    .init(
      id: id, row: .init(status: status, title: title, subtitle: subtitle, cost: cost),
      isReadOnly: isReadOnly)
  }
}
