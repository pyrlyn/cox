// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for `MenuBarPanel` (T51.13): mockup 26's panel with nothing waiting,
// and with an approval, a question and a running session. Separate so the organism's fixtures do
// not edit the shared file.

extension PreviewState {
  static let menuBarEmpty = MenuBarPanel.State(today: "$0.00 · 0 sessions")

  static let menuBarBusy = MenuBarPanel.State(
    needs: [
      .init(
        id: "call-1", session: "s-oauth", title: "Migrate auth to OAuth PKCE",
        kind: .approval(command: "git push -u origin wt/oauth-pkce")),
      .init(
        id: "call-2", session: "s-e2e", title: "Flaky e2e checkout test",
        kind: .question("retry flaky tests or quarantine?")),
    ],
    running: [
      .init(
        id: "s-jitter", title: "Add retry jitter to HTTP client", activity: "cargo nextest",
        elapsed: "12 s", cost: "$0.42")
    ],
    today: "$4.02 · 7 sessions")
}
