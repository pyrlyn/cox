// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the window shell's molecules (T37.21): the sidebar's sessions and
// filter and the toolbar's breadcrumb and capsules, as mockup screen 28 shows them. Separate from
// `PreviewState.swift` so molecules built in parallel add their fixtures without editing one file.

import SwiftUI

extension PreviewState {
  /// One session per status, the mockup's sidebar; the first is the open one.
  static let sessions: [SessionRow.Item] = [
    .init(
      status: .running, title: "Add retry jitter to HTTP client",
      subtitle: "cox · running cargo nextest", cost: "$0.42"),
    .init(
      status: .waiting, title: "Migrate auth to OAuth PKCE",
      subtitle: "acme-web · approve git push"),
    .init(
      status: .idle, title: "Bench plugin cold start", subtitle: "2 h ago · done", cost: "$1.18"),
    .init(
      status: .error, title: "Sitemap generator", subtitle: "failed · build error", cost: "$0.12"),
  ]

  static let filterPrompt = "Filter sessions"
  static let filterText = "retry"

  static let project = "cox"
  static let branch = "wt/retry-jitter"
  static let model = "Sonnet 5 · high"
  static let cost = "$0.42"
  static let context = "38%"
}

/// The sidebar's sessions stacked at the sidebar's width, none selected.
struct SessionList: View {
  var body: some View {
    VStack(spacing: 0) {
      ForEach(PreviewState.sessions, id: \.title) { SessionRow($0) }
    }
    .frame(width: Size.sidebarWidth)
  }
}

/// The toolbar's cost capsule with the mockup's figures.
struct CostCapsuleSample: View {
  let isOpen: Bool

  var body: some View {
    CostCapsule(
      cost: PreviewState.cost, context: PreviewState.context,
      fraction: PreviewState.fractions[1], isOpen: isOpen
    ) {}
  }
}

/// The open session's breadcrumb, with or without a branch.
struct BreadcrumbSample: View {
  let branch: String?

  var body: some View {
    Breadcrumb(PreviewState.sessions[0].title, project: PreviewState.project, branch: branch)
  }
}
