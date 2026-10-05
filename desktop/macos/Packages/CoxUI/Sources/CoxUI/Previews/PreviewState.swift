// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` (DS§5, DS§9): the fixtures and the frame every `#Preview` and snapshot test
// share, so a preview and its snapshot show the same values on the same backdrop and pane.
// The last layer: it may use every component, and nothing but a preview or a test uses it.

import SwiftUI

/// Values the previews and snapshots show.
enum PreviewState {
  static let keys = "⌘K"
  static let count = "3"
  static let sectionTitle = "Needs you"
  static let inlineCode = "cargo nextest run"
  static let risk = "network · writes remote"
  static let added = 42
  static let removed = 7

  /// A badge's text per kind, as the settings and agent rows show them.
  static func badge(_ kind: Badge.Kind) -> String {
    switch kind {
    case .neutral: "low · Haiku 4.5"
    case .user: "user"
    case .project: "project"
    case .env: "env"
    case .default: "default"
    case .warning: "deprecated"
    case .danger: "invalid"
    }
  }

  /// The DS§3.7 symbol a tool of each tile kind shows.
  static func symbol(_ kind: IconTile.Kind) -> String {
    switch kind {
    case .neutral: "globe"
    case .edit: "pencil"
    case .shell: "terminal"
    case .search: "magnifyingglass"
    case .write: "doc.text"
    }
  }
}

/// Colour behind every preview and snapshot, so glass and shadows show what they do to it.
struct PreviewBackdrop: View {
  var body: some View {
    LinearGradient(
      colors: [Color(.accent), Color(.statusPlan), Color(.statusSuccess)],
      startPoint: .topLeading, endPoint: .bottomTrailing)
  }
}

/// A sample on a chrome pane, as components sit in the app.
struct PreviewPane<Content: View>: View {
  @ViewBuilder let content: Content

  var body: some View {
    content
      .padding(Space.xl)
      .glassPane(RoundedRectangle(cornerRadius: Radius.pane, style: .continuous))
  }
}

/// A sample on a pane in every light/dark × Solid/Frosted cell (DS§9), for a `#Preview`.
struct PreviewMatrix<Content: View>: View {
  @ViewBuilder let content: () -> Content

  var body: some View {
    Grid(horizontalSpacing: Space.xl, verticalSpacing: Space.xl) {
      ForEach([ColorScheme.light, .dark], id: \.self) { scheme in
        GridRow {
          ForEach([GlassMaterial.solid, .frosted], id: \.self) { material in
            PreviewPane(content: content)
              .environment(\.colorScheme, scheme)
              .environment(\.coxAppearance, Appearance(material: material))
          }
        }
      }
    }
    .padding(Space.xxl)
    .background(PreviewBackdrop())
  }
}
