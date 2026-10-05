// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Settings window's state (DT§5.7): the view Rust built from the schema
// and the config layers, grouped for the sidebar, plus provider keys through
// a `SecretStore`. Every decision — the page, the label, the control, whether
// a field is read-only, whether a value or a key is taken (T58.4.8–T58.4.9) —
// already came from Rust; this store searches, sends edits and keeps the
// answer.

import CoxClient
import Foundation
import Observation

public struct SettingsSection: Identifiable, Equatable, Sendable {
  public let group: SettingsGroup
  public let settings: [Setting]
  public var id: SettingsGroup { group }
}

@Observable
@MainActor
public final class SettingsStore {
  public private(set) var view: SettingsView?
  /// Why the last load or edit failed; the next success clears it.
  public private(set) var failure: String?
  /// Why Rust refused the last rule edit or revoke (T37.45.3): the rule grammar's message, or a
  /// list a layer above the user file sets. The next one that succeeds clears it.
  public private(set) var ruleFailure: String?
  /// The providers whose key the `SecretStore` holds, read again after each load, store and
  /// removal so the Settings rows that show it redraw.
  public private(set) var storedKeys: Set<String> = []
  /// The sidebar's search: `sections` keeps only the settings whose label or dotted key holds
  /// it, so the pages and their boxes shrink to the matches. Empty keeps every setting.
  public var filter = ""
  /// The project whose layer applies.
  public let cwd: String
  @ObservationIgnored private let client: any SettingsClient
  @ObservationIgnored private let secrets: any SecretStore

  public init(client: any SettingsClient, secrets: any SecretStore, cwd: String) {
    (self.client, self.secrets, self.cwd) = (client, secrets, cwd)
  }

  public func load() async {
    await attempt { try await $0.client.settings(cwd: $0.cwd) }
  }

  public func set(_ key: String, to value: SettingValue) async {
    await attempt { try await $0.client.setSetting(cwd: $0.cwd, key: key, json: try value.json()) }
  }

  /// Sends what a control sent; Rust types it by the key's kind (a whole number for an integer,
  /// text parsed as a number) and its loader says why a value is refused.
  public func edit(_ key: String, _ input: SettingValue) async {
    await attempt {
      try await $0.client.setSettingInput(cwd: $0.cwd, key: key, input: input)
    }
  }

  /// Logs in to (`login`) or out of an MCP server, then reads its status back.
  public func setLogin(_ server: String, _ login: Bool) async {
    await attempt {
      try await $0.client.mcpLogin(cwd: $0.cwd, server: server, login: login)
      return try await $0.client.settings(cwd: $0.cwd)
    }
  }

  /// Adds (`old` nil), replaces or removes (`new` nil) one permission rule, in the user file only.
  public func editRule(_ kind: RuleKind, old: String?, new: String?) async {
    await attempt(\.ruleFailure) {
      try await $0.client.setPermissionRule(cwd: $0.cwd, kind: kind, old: old, new: new)
    }
  }

  /// Revokes an "allow for session" grant through its session's core.
  public func revoke(_ grant: SessionGrant) async {
    await attempt(\.ruleFailure) { try await $0.client.revokeGrant(cwd: $0.cwd, grant: grant) }
  }

  /// Non-empty groups in DT§5.7's order, keys sorted within each, narrowed to `filter`, which
  /// matches the core's title or the dotted key.
  public var sections: [SettingsSection] {
    let query = filter.trimmingCharacters(in: .whitespaces)
    let shown = (view?.settings ?? []).filter {
      query.isEmpty || $0.key.localizedStandardContains(query)
        || $0.title.localizedStandardContains(query)
    }
    let rows = Dictionary(grouping: shown) { $0.group }
    return SettingsGroup.allCases.compactMap { group in
      rows[group].map { SettingsSection(group: group, settings: $0) }
    }
  }

  /// The provider sections a key can be stored for, sorted.
  public var providers: [String] { view?.providers ?? [] }

  public func hasKey(for section: String) -> Bool { storedKeys.contains(section) }

  /// Stores `secret` as Rust trimmed it; Rust refuses an empty key or an unknown section.
  public func storeKey(_ secret: String, for section: String) throws {
    let secret = try client.checkKey(providers: providers, provider: section, secret: secret)
    try secrets.store(secret, for: section)
    readKeys()
  }

  public func removeKey(for section: String) throws {
    try secrets.remove(for: section)
    readKeys()
  }

  private func readKeys() {
    storedKeys = Set(providers.filter { ((try? secrets.secret(for: $0)) ?? nil) != nil })
  }

  private func attempt(
    _ failed: ReferenceWritableKeyPath<SettingsStore, String?> = \.failure,
    _ fetch: (SettingsStore) async throws -> SettingsView
  ) async {
    do {
      view = try await fetch(self)
      self[keyPath: failed] = nil
      readKeys()
    } catch {
      self[keyPath: failed] = String(describing: error)
    }
  }
}
