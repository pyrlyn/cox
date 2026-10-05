// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the sidebar's "Needs you" section as inbox rows (T37.27.7): two
// items of one session, a failure, and an expired approval shown read-only. Separate from
// `PreviewState+Window.swift` so this card adds its fixture without editing the shell's.

import SwiftUI

extension PreviewState {
  static let inboxSidebar = Sidebar.State(
    groups: [
      .init(
        id: "needs-you", title: "Needs you", kind: .section(count: "4"),
        sessions: [
          item("pkce#3", "pkce", .waiting, "bash git push", "approval waiting"),
          item("pkce#4", "pkce", .waiting, "Which branch?", "reviewer · question waiting"),
          item("seo#5", "seo", .error, "Rate limit reached", "turn failed"),
          item(
            "flaky#1", "flaky", .idle, "write checkout.spec.ts",
            "expired", isReadOnly: true),
        ])
    ],
    selection: "pkce", providers: "3 providers", providerStatus: .running)

  private static func item(
    _ id: String, _ session: String, _ status: StatusDot.Status, _ title: String,
    _ subtitle: String, isReadOnly: Bool = false
  ) -> Sidebar.Session {
    .init(
      id: id, row: .init(status: status, title: title, subtitle: subtitle, cost: nil),
      session: session, isReadOnly: isReadOnly)
  }
}
