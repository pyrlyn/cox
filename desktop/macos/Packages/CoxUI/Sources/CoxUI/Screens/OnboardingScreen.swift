// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `OnboardingScreen` (DS§6.5; DT§5.8): the first-run window — open a project, then the checklist
// from `cox doctor`'s checks (provider key, git, sandbox, login-shell environment), each saying
// what is missing and offering its fix. Composition only (DS§5): the rows, their status and what
// is missing arrive in `state` from `cox-app`'s checklist; the app binds `send`.

import SwiftUI

/// The checklist rows, in the order `cox-app` returns them.
public struct OnboardingScreenState: Equatable, Sendable {
  public var checks: [OnboardingScreen.Check]

  public init(checks: [OnboardingScreen.Check] = []) { self.checks = checks }
}

/// Every intent the first-run window reports.
public enum OnboardingScreenIntent: Equatable, Sendable {
  /// The folder picker, for the project the first session runs in.
  case chooseFolder
  /// Open this folder as the project: the one the picker chose, or one dropped on the step.
  case openFolder(URL)
  /// Settings, to store a provider key.
  case openSettings
  /// Run the checks again, after the person fixed something outside the app.
  case retry
}

/// The first-run window: the `ProjectDropZone` step over a `SettingsGroupBox` of `ChecklistRow`s.
public struct OnboardingScreen: View {
  let state: OnboardingScreenState
  let send: (OnboardingScreenIntent) -> Void

  public init(state: OnboardingScreenState, send: @escaping (OnboardingScreenIntent) -> Void) {
    (self.state, self.send) = (state, send)
  }

  public var body: some View {
    ShellPane(.window) {
      ScrollView {
        // The mockup's first-run window heads neither the drop zone nor the checklist: each row
        // names itself.
        VStack(alignment: .leading, spacing: Space.xxl) {
          ProjectDropZone(send: send)
          SettingsGroupBox(nil) {
            ForEach(state.checks) { check in
              ChecklistRow(
                check.title, detail: check.detail, status: check.status, action: check.fix?.title
              ) { if let fix = check.fix { send(fix.intent) } }
            }
          }
        }
        .frame(maxWidth: Size.readingWidth)
        .padding(Space.huge)
        .frame(maxWidth: .infinity)
      }
    }
  }
}

extension OnboardingScreen {
  public struct Check: Identifiable, Equatable, Sendable {
    /// The check's stable id from `cox-app` (`provider_key`, `git`, `sandbox`, `shell_env`).
    public let id: String
    var title: String
    /// What was found, or what is missing.
    var detail: String
    var status: ChecklistRow.Status
    var fix: Fix?

    public init(
      id: String, title: String, detail: String, status: ChecklistRow.Status, fix: Fix? = nil
    ) {
      (self.id, self.title, self.detail, self.status, self.fix) = (id, title, detail, status, fix)
    }
  }

  /// The button a check that is not ok offers.
  public enum Fix: Equatable, Sendable {
    case openSettings, retry

    var title: String {
      switch self {
      case .openSettings: "Open Settings"
      case .retry: "Check Again"
      }
    }

    var intent: OnboardingScreenIntent {
      switch self {
      case .openSettings: .openSettings
      case .retry: .retry
      }
    }
  }
}

#Preview("no provider") {
  OnboardingScreen(state: PreviewState.onboardingNoProvider) { _ in }
    .frame(width: Size.windowSmallWidth, height: Size.windowMinHeight)
    .padding(Space.xxl)
    .background(PreviewBackdrop())
}

#Preview("all green") {
  OnboardingScreen(state: PreviewState.onboardingAllGreen) { _ in }
    .frame(width: Size.windowSmallWidth, height: Size.windowMinHeight)
    .padding(Space.xxl)
    .background(PreviewBackdrop())
}
