// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The agent a new session is opened with (T52.7, DT§3.3.1): the New-session sheet's list and
// the one chosen, carried into the `OpenSession` the window hands the core. Here, not in the
// sheet, so the rule that an agent that cannot start is never chosen is tested without a view.

import CoxClient
import Observation

@MainActor
@Observable
public final class AgentPicker {
  /// Cox first, then each external agent, as the core lists them.
  public private(set) var choices: [AgentChoice] = [.cox]
  /// The agent picked; `nil` is cox.
  public private(set) var chosen: String?
  /// Why the list could not be read; the picker then offers cox alone.
  public private(set) var failure: String?

  private let client: any AgentsClient

  public init(client: any AgentsClient) { self.client = client }

  /// Reads the list for a session in `cwd`. A chosen agent that is gone or can no longer start
  /// falls back to cox.
  public func load(cwd: String) async {
    do {
      choices = try await client.agents(cwd: cwd)
      failure = nil
    } catch {
      (choices, failure) = ([.cox], String(describing: error))
    }
    if !choices.contains(where: { $0.name == chosen && $0.isAvailable }) { chosen = nil }
  }

  /// Picks `name` (`nil` for cox); an agent that cannot start is refused and the pick stays.
  @discardableResult
  public func choose(_ name: String?) -> Bool {
    guard let row = choices.first(where: { $0.name == name }), row.isAvailable else {
      return false
    }
    chosen = name
    return true
  }

  /// The row picked, for the sheet's label.
  public var selection: AgentChoice { choices.first { $0.name == chosen } ?? .cox }

  /// What the window opens: a new session in `cwd`, driven by the agent picked.
  public func request(cwd: String, theme: String) -> OpenSession {
    OpenSession(cwd: cwd, theme: theme, agent: chosen)
  }
}

extension [AgentChoice] {
  /// What the UI calls the agent `name`: its row's label, or the config name when the list does
  /// not have it (not read yet, or the agent was removed from the config since).
  public func label(of name: String) -> String { first { $0.name == name }?.label ?? name }
}
