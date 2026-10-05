// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Settings screen's values (DT§5.7): every config leaf with the layer it
// came from, the page, box, label and control Rust gave it, field for field as
// cox-ffi exports `cox_app::settings`, and the `SettingsClient` seam the store
// reads them through. Separate from the timeline because Settings is its own
// window with its own client; `LiveCoreClient` (CoxCore) is the Rust side,
// `FixtureSettingsClient` the one previews and tests use (DT§8).

import Foundation
import Synchronization

/// Where a value came from: the badge beside each field.
public enum Layer: String, Equatable, Sendable {
  case `default`, user, project, env, flag
  case claudeSettings = "claude-settings"
}

/// The control a field takes, from `docs/config.jsonschema`.
public enum SettingKind: Equatable, Sendable {
  case toggle
  case integer(min: Double?, max: Double?)
  case number(min: Double?, max: Double?)
  case text
  case choice(options: [String])
  case list
  /// A shape the schema leaves open (plugin tables, hook entries).
  case other
}

/// The sidebar pages DT§5.7 names, in its order; Rust puts each key on one.
public enum SettingsGroup: String, CaseIterable, Sendable {
  case general, models, permissions, sandbox, budget, mcp, plugins, appearance, advanced
}

public struct Setting: Identifiable, Equatable, Sendable {
  /// What a field shows, as Rust chose it from the key's kind and value.
  public enum Control: Equatable, Sendable {
    case toggle(Bool)
    /// A number the schema bounds on both ends, and the value as its JSON reads.
    case slider(Double, range: ClosedRange<Double>, text: String)
    /// A few options side by side.
    case choice(String, options: [String])
    /// A pop-up: more options than fit side by side, or a tier's model from the catalog.
    case menu(String, options: [Option])
    /// Text, or a number typed as text; Rust types it back (`setSettingInput`).
    case field(String)
    /// A list or an open shape, as its JSON: Settings does not edit it.
    case json(String)
  }

  /// A pop-up's option: the value sent, and what the menu calls it.
  public struct Option: Equatable, Sendable {
    public let value: String
    public let title: String

    public init(value: String, title: String) { (self.value, self.title) = (value, title) }
  }

  /// Dotted, as `cox config set` takes it.
  public var key: String
  /// JSON text.
  public var value: String
  public var layer: Layer
  /// `false` once a layer above the user file sets the key: an edit there
  /// would not take effect, so the field is read-only (DT§5.7).
  public var editable: Bool
  public var kind: SettingKind
  public var description: String
  /// The page it is on.
  public var group: SettingsGroup
  /// Its label, `base_url` → `Base url`.
  public var title: String
  /// The box it sits in, its table; `nil` for a rule list.
  public var table: String?
  /// For a `providers.<name>` table's key, the section whose key the box takes.
  public var provider: String?
  /// `Set in <project file>` for a project value, else the schema's help.
  public var detail: String?
  public var control: Control

  public var id: String { key }

  /// The layout fields default for test values that only need the first six.
  public init(
    key: String, value: String, layer: Layer, editable: Bool, kind: SettingKind,
    description: String, group: SettingsGroup = .advanced, title: String = "",
    table: String? = nil, provider: String? = nil, detail: String? = nil,
    control: Control = .json("")
  ) {
    (self.key, self.value, self.layer, self.editable) = (key, value, layer, editable)
    (self.kind, self.description, self.group, self.title) = (kind, description, group, title)
    (self.table, self.provider, self.detail, self.control) = (table, provider, detail, control)
  }
}

/// A typed edit, sent to Rust, which types it again by the key's kind (`setSettingInput`).
public enum SettingValue: Equatable, Sendable, Encodable {
  case bool(Bool)
  case integer(Int64)
  case number(Double)
  case text(String)
  case list([String])

  public func encode(to encoder: any Encoder) throws {
    var container = encoder.singleValueContainer()
    switch self {
    case .bool(let value): try container.encode(value)
    case .integer(let value): try container.encode(value)
    case .number(let value): try container.encode(value)
    case .text(let value): try container.encode(value)
    case .list(let value): try container.encode(value)
    }
  }

  /// Throws for a number JSON cannot carry (NaN, infinity).
  public func json() throws -> String {
    guard let text = String(bytes: try JSONEncoder().encode(self), encoding: .utf8) else {
      throw EncodingError.invalidValue(self, .init(codingPath: [], debugDescription: "not UTF-8"))
    }
    return text
  }
}

/// Why Rust would not store a provider key (`checkKey`).
public enum KeyError: Error, Equatable {
  case empty
  /// Not a `[providers.<name>]` table in the current view.
  case unknownProvider(String)
}

public struct SettingsView: Equatable, Sendable {
  /// Sorted by key.
  public var settings: [Setting]
  /// Where edits go.
  public var userFile: String
  /// The project's `.cox/config.toml`, when there is one.
  public var projectFile: String?
  /// The MCP servers in effect and their logins, sorted by name.
  public var mcp: [McpServer]
  /// Project values the guard list threw out.
  public var dropped: [Dropped]
  /// The allow/ask/deny rules in effect, deny first.
  public var rules: [PermissionRule]
  /// The "allow for session" grants of the sessions open here.
  public var grants: [SessionGrant]
  /// The provider sections a key can be stored for, sorted.
  public var providers: [String]

  public init(
    settings: [Setting], userFile: String, projectFile: String? = nil, mcp: [McpServer] = [],
    dropped: [Dropped] = [], rules: [PermissionRule] = [], grants: [SessionGrant] = [],
    providers: [String] = []
  ) {
    (self.settings, self.userFile, self.projectFile, self.mcp) = (
      settings, userFile, projectFile, mcp
    )
    (self.dropped, self.rules, self.grants, self.providers) = (dropped, rules, grants, providers)
  }
}

public protocol SettingsClient: Sendable {
  /// The effective config for a session in `cwd`.
  func settings(cwd: String) async throws -> SettingsView
  /// Writes `json` for `key` to the user file; the view after the edit.
  /// Rust refuses a read-only key and a value the config loader rejects.
  func setSetting(cwd: String, key: String, json: String) async throws -> SettingsView
  /// Sets `key` from what its control sent; Rust types `input` by the key's kind first (an
  /// integer rounded, text parsed as a number, else text). The view after the edit.
  func setSettingInput(cwd: String, key: String, input: SettingValue) async throws -> SettingsView
  /// `secret` trimmed, to store for `provider`; throws `KeyError` for an empty key or a section
  /// `providers` (the view's) does not list.
  func checkKey(providers: [String], provider: String, secret: String) throws -> String
  /// Logs in to (`login`) or out of the MCP server `server`; a login's page goes to the host's
  /// `open` and the call returns once the browser comes back.
  func mcpLogin(cwd: String, server: String, login: Bool) async throws
  /// Adds (`old` nil), replaces or removes (`new` nil) one rule of `kind` in the user file; the
  /// view after it. Rust checks `new` with the permission engine's grammar and refuses a list a
  /// layer above the user file sets, both as `RuleRefused`.
  func setPermissionRule(cwd: String, kind: RuleKind, old: String?, new: String?) async throws
    -> SettingsView
  /// Revokes `grant` through its session's core, which asks again for the next call it covered.
  func revokeGrant(cwd: String, grant: SessionGrant) async throws -> SettingsView
}

/// A fixed view that takes edits the way Rust does for an editable key:
/// the value changes and its layer becomes `user`. Keeps what it was sent.
/// A login opens `loginPage` through `host`, then its callback is scripted: the
/// server is logged in with an hour left. A rule edit lands in the user layer unless its list is
/// read-only or `ruleRefusal` is set; a revoke drops the grant.
public final class FixtureSettingsClient: SettingsClient {
  public struct ReadOnly: Error, Equatable { public let key: String }
  public struct NoLogin: Error, Equatable { public let server: String }

  public static let loginPage = "https://auth.example.test/authorize?client_id=cox"

  private let state: Mutex<(view: SettingsView, sent: [String])>
  private let host: (any PlatformHost)?

  /// What every rule edit is refused with, as Rust refuses a rule its grammar rejects.
  private let ruleRefusal: String?

  public init(
    view: SettingsView, host: (any PlatformHost)? = nil, ruleRefusal: String? = nil
  ) {
    state = Mutex((view, []))
    (self.host, self.ruleRefusal) = (host, ruleRefusal)
  }

  /// `key=json`, in order.
  public var sent: [String] { state.withLock { $0.sent } }

  public func settings(cwd: String) async throws -> SettingsView {
    state.withLock { $0.view }
  }

  public func setSetting(cwd: String, key: String, json: String) async throws -> SettingsView {
    try state.withLock { state in
      guard let index = state.view.settings.firstIndex(where: { $0.key == key }),
        state.view.settings[index].editable
      else { throw ReadOnly(key: key) }
      state.sent.append("\(key)=\(json)")
      state.view.settings[index].value = json
      state.view.settings[index].layer = .user
      return state.view
    }
  }

  /// Sends `input` as its own JSON: typing it by the key's kind is Rust's.
  public func setSettingInput(
    cwd: String, key: String, input: SettingValue
  ) async throws -> SettingsView {
    try await setSetting(cwd: cwd, key: key, json: try input.json())
  }

  /// Enough to drive the store: Rust's `check_key` is the rule (T58.4.8).
  public func checkKey(providers: [String], provider: String, secret: String) throws -> String {
    let secret = secret.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !secret.isEmpty else { throw KeyError.empty }
    guard providers.contains(provider) else { throw KeyError.unknownProvider(provider) }
    return secret
  }

  public func mcpLogin(cwd: String, server: String, login: Bool) async throws {
    let index = state.withLock { state in
      state.view.mcp.firstIndex { $0.name == server && $0.login != .stdio }
    }
    guard let index else { throw NoLogin(server: server) }
    if login { host?.open(Self.loginPage) }
    state.withLock { state in
      state.sent.append("\(login ? "login" : "logout")=\(server)")
      state.view.mcp[index].fixtureLogin(login)
    }
  }

  public func setPermissionRule(
    cwd: String, kind: RuleKind, old: String?, new: String?
  ) async throws -> SettingsView {
    if let ruleRefusal { throw RuleRefused(ruleRefusal) }
    return try state.withLock { state in
      var list = state.view.rules.filter { $0.kind == kind }
      guard list.allSatisfy(\.editable) else { throw RuleRefused("set by a higher layer") }
      if let old {
        guard let index = list.firstIndex(where: { $0.rule == old }) else {
          throw RuleRefused("no rule `\(old)`")
        }
        if let new { list[index].rule = new } else { list.remove(at: index) }
      } else if let new {
        list.append(.init(kind: kind, rule: new, layer: .user, editable: true))
      }
      state.sent.append("\(kind.rawValue)=\(old ?? "")>\(new ?? "")")
      let edited = list.map {
        PermissionRule(kind: kind, rule: $0.rule, layer: .user, editable: true)
      }
      // Rust's order: deny, then allow, then ask.
      let rules = state.view.rules
      state.view.rules = [RuleKind.deny, .allow, .ask].flatMap { other in
        other == kind ? edited : rules.filter { $0.kind == other }
      }
      return state.view
    }
  }

  public func revokeGrant(cwd: String, grant: SessionGrant) async throws -> SettingsView {
    state.withLock { state in
      state.sent.append("revoke=\(grant.tool) \(grant.subject)")
      state.view.grants.removeAll { $0 == grant }
      return state.view
    }
  }
}
