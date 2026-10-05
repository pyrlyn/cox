// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// "Ask cox in <project>" (T51.17): a new session in the project, then the prompt sent as the
// composer's Send, so the turn runs like any other; its approvals land in the inbox and the
// engine decides as always. Here, not in the app's intent, so open-then-send is tested without
// App Intents. The registry holds the session for a hand-off token until a window joins it.

import CoxClient
import Foundation

public struct AskCox: Equatable, Sendable {
  /// The project's root: the new session's cwd.
  public let project: String
  public let prompt: String

  public init(project: String, prompt: String) { (self.project, self.prompt) = (project, prompt) }

  public func request(theme: String) -> OpenSession { OpenSession(cwd: project, theme: theme) }

  /// The composer's Send, with no attachments.
  public var intent: Intent { .send(text: prompt, attachments: []) }

  /// Opens the session on `core`, holds it in `registry` for `token` and sends the prompt. The
  /// window that shows it joins, then releases `token`; a send that fails lets the session go.
  @MainActor
  public func run(
    on core: any CoreClient, registry: AppStore, token: UUID, theme: String
  ) async throws -> AppStore.Shared {
    let client = try await core.open(request(theme: theme))
    let shared = registry.adopt(client, window: token)
    do {
      _ = try await shared.store.send(intent)
    } catch {
      registry.release(client.id, window: token)
      throw error
    }
    return shared
  }
}
