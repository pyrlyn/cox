// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The App Intents (T51.17): "Ask cox" starts a session in a project and sends the prompt as the
// composer's Send (CoxModel's `AskCox`), so the engine decides and approvals reach the inbox as
// for any turn; "Open session" opens one. Both bring cox forward and show the session in a
// window of its own. Separate from the entities and the shortcuts, which only describe them.

import AppIntents
import CoxModel
import Foundation

struct AskCoxIntent: AppIntent {
  static let title: LocalizedStringResource = "Ask cox"
  static let description = IntentDescription("Starts a session in a project and sends it a prompt.")
  static let openAppWhenRun = true

  @Parameter(title: "Project") var project: ProjectEntity
  @Parameter(title: "Prompt") var prompt: String
  @Dependency private var model: AppModel

  static var parameterSummary: some ParameterSummary {
    Summary("Ask cox in \(\.$project): \(\.$prompt)")
  }

  @MainActor
  func perform() async throws -> some IntentResult {
    // As a window's first session does: the login shell's environment before the core opens one.
    await model.loadLoginEnv()
    let token = UUID()
    let shared = try await AskCox(project: project.id, prompt: prompt).run(
      on: model.launch.core.get(), registry: model.registry, token: token,
      theme: SessionWindow.syntaxTheme)
    model.show(PopOut(session: shared.store.session.id, handoff: token))
    return .result()
  }
}

struct OpenSessionIntent: AppIntent {
  static let title: LocalizedStringResource = "Open session"
  static let description = IntentDescription("Opens a cox session in a window of its own.")
  static let openAppWhenRun = true

  @Parameter(title: "Session") var session: SessionEntity
  @Dependency private var model: AppModel

  @MainActor
  func perform() async throws -> some IntentResult {
    model.show(PopOut(session: session.id, asTab: false))
    return .result()
  }
}
