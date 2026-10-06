// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The composer's model popover (DT§5.1, DT§5.3, T37.22.6, T60.7): the core's sections, each by
// the provider it lists (A138), each row by the core's short name (A111, A129), the one the
// session runs on marked. A pick is `/model <tier> <id>`, the TUI's switch, or — for a model of
// another provider than the session's — `Intent.switchProvider`; a section whose provider has no
// key stays listed but its rows do not pick. Here, not in CoxUI, because the store owns what the
// menu shows (DS§1); the app copies it into `ModelPopover.State`.

import CoxClient

public struct ModelMenu: Equatable, Sendable {
  public struct Row: Identifiable, Equatable, Sendable {
    /// `code/<provider>/<model>`: the same model id under two providers is two rows. A
    /// section a fixture gives no provider keeps the old `code/<model>`.
    public let id: String
    public let tier: Tier
    /// The id the switch sends.
    public let model: String
    /// What the row shows: the core's short name, `Sonnet 5`, else the id.
    public let name: String
    /// `low · high`: the efforts it takes; empty when it takes any.
    public let detail: String
    public let isSelected: Bool
  }

  public struct Section: Identifiable, Equatable, Sendable {
    public var id: String { title }
    /// `Code`, `Think`, `Cheap` for a tier's own provider, the provider's name for one only
    /// configured.
    public let title: String
    public let tier: Tier
    /// The `[providers.<name>]` section whose models these are; empty on a fixture that omits it.
    public let provider: String
    /// Whether a turn could run on that provider now. When not, the header offers "Add key" and
    /// the rows are greyed.
    public let usable: Bool
    /// Why a usable section's rows still do not pick: a remote session cannot move to another
    /// provider.
    public let unavailable: String?
    public let rows: [Row]

    /// Its rows pick.
    public var isEnabled: Bool { usable && unavailable == nil }
  }

  public var sections: [Section] = []
  /// The provider the session runs on; `nil` before the core reports it, when any row is the
  /// session's own.
  private var provider: String?

  public init() {}

  /// The core's sections as they are (`App.modelMenu`), the running model marked.
  /// `canSwitchProvider` is the session's (`SessionClient.canSwitchProvider`).
  public init(sections: [ModelSection], status: Status, canSwitchProvider: Bool = true) {
    provider = status.provider
    self.sections = sections.map { section in
      let other = Self.isOther(section.tier, section.provider, than: status.provider)
      return Section(
        title: section.title, tier: section.tier, provider: section.provider,
        usable: section.usable,
        unavailable: other && !canSwitchProvider
          ? "A remote session cannot change provider" : nil,
        rows: section.models.map { model in
          Row(
            id: section.provider.isEmpty
              ? "\(section.tier.rawValue)/\(model.id)"
              : "\(section.tier.rawValue)/\(section.provider)/\(model.id)",
            tier: section.tier, model: model.id,
            name: model.shortName ?? model.id, detail: model.efforts,
            isSelected: model.id == status.model
              && (section.provider.isEmpty || status.provider.map { $0 == section.provider } ?? true))
        })
    }
  }

  /// A code-tier section of a provider the session does not run on: a pick moves the whole
  /// session. A think or cheap tier's own provider is a plain tier switch, whichever it is.
  private static func isOther(_ tier: Tier, _ provider: String, than session: String?) -> Bool {
    guard tier == .code, let session, !provider.isEmpty else { return false }
    return provider != session
  }

  /// What a row's click sends: the tier's model switch, or — for another provider's model — the
  /// provider switch, which reopens the session before its first turn. `nil` for a row the menu
  /// does not list or one that does not pick.
  public func pick(_ row: Row.ID) -> Intent? {
    guard let section = sections.first(where: { $0.rows.contains { $0.id == row } }),
      section.isEnabled, let picked = section.rows.first(where: { $0.id == row })
    else { return nil }
    if Self.isOther(section.tier, section.provider, than: provider) {
      return .switchProvider(provider: section.provider, model: picked.model, makeDefault: false)
    }
    return .switchModel(tier: picked.tier, model: picked.model)
  }
}
