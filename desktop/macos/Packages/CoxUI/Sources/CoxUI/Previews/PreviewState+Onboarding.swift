// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the first-run window (T37.31, DT§5.8): mockup screen 21's
// checklist with no provider key stored, and with every check passing, in the words
// `cox doctor`'s checks use. Separate so this card adds its fixtures without editing another's.

import SwiftUI

extension PreviewState {
  static let checksTitle = "Checks"

  /// The code tier's provider has no key; everything else passes.
  static let onboardingNoProvider = OnboardingScreenState(checks: [
    .init(
      id: "provider_key", title: "Provider key",
      detail: "ANTHROPIC_API_KEY is not set and keyring entry 'cox/anthropic' not found",
      status: .missing, fix: .openSettings),
    git, sandbox, shell,
  ])

  /// Every check passes.
  static let onboardingAllGreen = OnboardingScreenState(checks: [
    .init(
      id: "provider_key", title: "Provider key", detail: "anthropic key found", status: .passed),
    git, sandbox, shell,
  ])

  private static let git = OnboardingScreen.Check(
    id: "git", title: "Git", detail: "git version 2.51.0", status: .passed)
  private static let sandbox = OnboardingScreen.Check(
    id: "sandbox", title: "Sandbox", detail: "seatbelt", status: .passed)
  private static let shell = OnboardingScreen.Check(
    id: "shell_env", title: "Shell environment",
    detail: "PATH and variables from your login shell", status: .passed)

  /// A row's words per status, as the checklist shows them.
  static func check(_ status: ChecklistRow.Status) -> (title: String, detail: String) {
    switch status {
    case .passed: ("Git", "git version 2.51.0")
    case .warning: ("Shell environment", "zsh took longer than 10 s; using the app's own PATH")
    case .missing: ("Provider key", "ANTHROPIC_API_KEY is not set")
    case .step: ("Open a project", "Choose a folder. A git repository is recommended.")
    }
  }
}

/// One checklist row per status; all but the passing one offer a fix.
struct ChecklistRowSample: View {
  let status: ChecklistRow.Status

  var body: some View {
    let words = PreviewState.check(status)
    ChecklistRow(
      words.title, detail: words.detail, status: status,
      action: status == .passed ? nil : status == .step ? "Choose Folder…" : "Check Again"
    )
    .frame(width: Size.readingWidth)
  }
}
