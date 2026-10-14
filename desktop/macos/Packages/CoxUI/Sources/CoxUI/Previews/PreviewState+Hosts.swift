// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for remote hosts (T52.21): a local project, then a connected host and a
// disconnected one, each a sidebar group under its host badge; and the Connect to Target sheet
// (T68.5) listing, after a failed connect and on `hello`. Separate from `PreviewState+Window.swift`
// so this card adds its fixtures without editing the shell's.

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

  static let connectTargetListed = ConnectTargetSheet.State(
    kinds: connectKinds,
    options: [
      .init(id: "ssh:devbox", kind: "ssh", name: "devbox", isConfigured: true),
      .init(id: "docker:web", kind: "docker", name: "web", isConfigured: true),
      .init(id: "docker:db-1", kind: "docker", name: "db-1", isConfigured: false),
    ])

  static let connectTargetFailed = ConnectTargetSheet.State(
    kinds: connectKinds, options: connectTargetListed.options,
    failure: "ssh: Could not resolve hostname devbox: nodename nor servname provided")

  static let connectTargetHello = ConnectTargetSheet.State(
    kinds: connectKinds, options: connectTargetListed.options,
    hello: .init(summary: "Linux · arm64 · cox 0.9.1", sandbox: "bwrap"))

  static let connectTargetNoSandbox = ConnectTargetSheet.State(
    kinds: connectKinds, options: connectTargetListed.options,
    hello: .init(summary: "Linux · x86_64 · cox 0.9.1", sandbox: nil))

  private static let connectKinds: [ConnectTargetSheet.Kind] = [
    .init(id: "ssh", title: "SSH", example: "devbox"),
    .init(id: "docker", title: "Docker", example: "web"),
    .init(id: "lima", title: "Lima", example: "default"),
    .init(id: "colima", title: "Colima", example: "default"),
  ]

  private static func hostRow(
    _ id: String, _ status: StatusDot.Status, _ title: String, _ subtitle: String,
    cost: String? = nil, isReadOnly: Bool = false
  ) -> Sidebar.Session {
    .init(
      id: id, row: .init(status: status, title: title, subtitle: subtitle, cost: cost),
      isReadOnly: isReadOnly)
  }
}
