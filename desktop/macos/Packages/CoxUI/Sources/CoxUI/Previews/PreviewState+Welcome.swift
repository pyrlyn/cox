// `PreviewState` fixtures for Figma frames 22-empty-session (the welcome hero) and
// 14-rewind-edit-resend (the turn gutter and the rewind menu), with the frames' own words.
// Separate so organisms built in parallel add their fixtures without editing one file.

import SwiftUI

extension PreviewState {
  /// Frame 22's hero over the cox workspace.
  static let welcome = WelcomeHero.State(
    project: "cox", summary: "Rust workspace · 31 crates · AGENTS.md loaded",
    suggestions: [
      .init(
        title: "Explain the architecture",
        detail: "How do cox-core, cox-protocol and the surfaces fit together?",
        prompt:
          "Explain the architecture: how do cox-core, cox-protocol and the surfaces fit together?"),
      .init(
        title: "Find and fix a failing test", detail: "Run cargo nextest and fix the first failure",
        prompt: "Run cargo nextest and fix the first failure."),
      .init(
        title: "Review my uncommitted diff", detail: "Check git diff for bugs before I commit",
        prompt: "Review my uncommitted diff: check git diff for bugs before I commit."),
    ])

  /// Frame 14's menu: turn 2, two files restored, the first scope highlighted.
  static let rewindMenu = RewindMenu.State(turn: 2, restoredFiles: 2)
}

/// The hero in a reading column as tall as the frame's, on the transcript pane.
struct WelcomeHeroSample: View {
  let state: WelcomeHero.State

  var body: some View {
    ShellPane(.column) { WelcomeHero(state: state) { _ in } }
      .frame(width: Size.readingWidth + Space.huge * 2, height: Size.welcomeTop * 3)
  }
}
