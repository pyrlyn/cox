// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the window shell's organisms and screen (T37.22): the sidebar,
// toolbar and main screen of mockup screens 28–29, with the transcript and inspector slots
// left empty for their own cards. Separate from `PreviewState+Shell.swift`, which holds the
// shell molecules' fixtures, so each card adds its fixtures without editing another's file.

import SwiftUI

extension PreviewState {
  /// The mockup's window, 1440 × 900 pt.
  private static let windowWidth: CGFloat = 1440
  private static let windowHeight: CGFloat = 900
  static let window = CGSize(width: windowWidth, height: windowHeight)

  /// The mockup's sidebar: two status sections, two open projects and a folded one.
  static let sidebar = Sidebar.State(
    groups: [
      .init(
        id: "needs-you", title: "Needs you", kind: .section(count: "2"),
        sessions: [
          session("pkce", sessions[1]),
          session(
            "flaky", .waiting, "Flaky e2e checkout test", "acme-web · question waiting"),
        ]),
      .init(
        id: "running", title: "Running", kind: .section(count: nil),
        sessions: [session("jitter", sessions[0])]),
      .init(
        id: "cox", title: "cox", kind: .project(isExpanded: true),
        sessions: [
          session("bench", sessions[2]),
          session("clippy", .idle, "Fix clippy on nightly", "yesterday · done", "$0.09"),
          session("explain", .idle, "Explain compaction", "Mon · done", "$0.21"),
        ]),
      .init(
        id: "acme-web", title: "acme-web", kind: .project(isExpanded: true),
        sessions: [
          session("dark", .idle, "Dark mode toggle", "3 h ago · done", "$0.64"),
          session("seo", sessions[3]),
        ]),
      .init(
        id: "dotfiles", title: "dotfiles", kind: .project(isExpanded: false),
        sessions: [session("zsh", .idle, "Tidy zsh startup", "last week · done", "$0.03")]),
    ],
    selection: "jitter", providers: "3 providers", providerStatus: .running)

  /// The mockup's toolbar while the turn runs.
  static let toolbar = SessionToolbar.State(
    title: sessions[0].title, project: project, branch: branch, model: model, cost: cost,
    context: context, contextFraction: fractions[1], isRunning: true)

  static let main = MainScreenState(sidebar: sidebar, toolbar: toolbar)

  /// The same window with the sidebar and the inspector folded away.
  static let folded = MainScreenState(
    sidebar: sidebar, toolbar: toolbar, isSidebarVisible: false, isInspectorVisible: false)

  private static func session(_ id: String, _ row: SessionRow.Item) -> Sidebar.Session {
    .init(id: id, row: row)
  }

  private static func session(
    _ id: String, _ status: StatusDot.Status, _ title: String, _ subtitle: String,
    _ cost: String? = nil
  ) -> Sidebar.Session {
    .init(id: id, row: .init(status: status, title: title, subtitle: subtitle, cost: cost))
  }
}

/// The main screen with its transcript and inspector slots empty, as this card leaves them.
struct MainScreenSample: View {
  let state: MainScreenState

  var body: some View {
    MainScreen(state: state, send: { _ in }, transcript: { EmptyView() }, inspector: { _ in })
  }
}
