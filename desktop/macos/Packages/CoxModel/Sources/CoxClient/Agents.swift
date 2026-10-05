// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Who can drive a new session (DT§3.3.1, T52.7): cox itself and every external ACP agent the
// config or a granted plugin names, field for field as cox-ffi exports `cox_app::AgentChoice`.
// Separate from the session seam because it is read from the config for a cwd, before any
// session opens.

/// `cox_app::AgentChoice`: one row of the New-session picker and of the Agents list.
public struct AgentChoice: Equatable, Sendable, Identifiable {
  /// What `OpenSession.agent` takes; `nil` for cox itself.
  public var name: String?
  /// What the UI calls it: "Claude Agent", never "Claude Code".
  public var label: String
  /// `built-in`, `user config` or `plugin <id>`.
  public var origin: String
  /// The program, args, key, network and writable directories; empty for cox.
  public var launch: String
  /// Why it cannot start here; `nil` when it can.
  public var unavailable: String?

  public var id: String { name ?? "" }
  public var isAvailable: Bool { unavailable == nil }

  public init(
    name: String?, label: String, origin: String = "built-in", launch: String = "",
    unavailable: String? = nil
  ) {
    (self.name, self.label, self.origin, self.launch, self.unavailable) =
      (name, label, origin, launch, unavailable)
  }

  /// The built-in row the core lists first.
  public static let cox = AgentChoice(name: nil, label: "cox")
}

/// The agents half of cox-ffi's `App`.
public protocol AgentsClient: Sendable {
  /// Cox first, then each external agent, for a session in `cwd`. Loads the granted plugins,
  /// so it waits.
  func agents(cwd: String) async throws -> [AgentChoice]
}

/// A fixed list: enough to drive the picker in a test or a preview.
public struct FixtureAgents: AgentsClient {
  public var fixed: [AgentChoice]

  public init(_ fixed: [AgentChoice] = [.cox]) { self.fixed = fixed }

  public func agents(cwd: String) async -> [AgentChoice] { fixed }
}
