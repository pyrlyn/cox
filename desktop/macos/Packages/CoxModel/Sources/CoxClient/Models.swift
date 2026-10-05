// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// What the toolbar's model popover offers, as the catalog and as the core's sections (T58.4.7),
// and which providers can answer (DT§5.1, T37.22.6, A110), field for field as cox-ffi exports
// `cox_app::models`. Separate from the session seam
// because both are read from the config for a cwd, before or beside any open session.

/// `cox_app::ModelChoice`: one model a tier can switch to, as `/model <tier> <id>` names it.
public struct ModelChoice: Equatable, Sendable {
  public var tier: Tier
  /// The `[providers.<name>]` section the tier calls.
  public var provider: String
  /// The id sent on the wire.
  public var id: String
  /// What the catalog calls it, `Claude Sonnet 5`; `nil` when it has no name.
  public var displayName: String?
  /// That name as the desktop shows it, `Sonnet 5` (A129); `nil` with `displayName`.
  public var shortName: String?
  /// The efforts it takes; empty means any.
  public var efforts: [Effort]
  public var contextWindow: UInt32?

  public init(
    tier: Tier, provider: String, id: String, displayName: String? = nil,
    shortName: String? = nil, efforts: [Effort] = [], contextWindow: UInt32? = nil
  ) {
    (self.tier, self.provider, self.id, self.displayName) = (tier, provider, id, displayName)
    (self.shortName, self.efforts, self.contextWindow) = (shortName, efforts, contextWindow)
  }
}

/// `cox_app::ModelSection`: one popover section, a tier's models not listed by an earlier one.
public struct ModelSection: Equatable, Sendable {
  public var tier: Tier
  /// `Code`, `Think`, `Cheap`.
  public var title: String
  public var models: [MenuModel]

  public init(tier: Tier, title: String, models: [MenuModel]) {
    (self.tier, self.title, self.models) = (tier, title, models)
  }
}

/// `cox_app::MenuModel`: a popover row; which one runs is the client's to mark.
public struct MenuModel: Equatable, Sendable {
  /// The id the switch sends.
  public var id: String
  /// What the catalog calls it; `nil` when it has no name.
  public var displayName: String?
  /// What the row shows, `Sonnet 5` (A129); `nil` with `displayName`, and the row shows the id.
  public var shortName: String?
  /// `low · high`; empty when it takes any effort.
  public var efforts: String

  public init(
    id: String, displayName: String? = nil, shortName: String? = nil, efforts: String = ""
  ) {
    (self.id, self.displayName, self.shortName, self.efforts) = (
      id, displayName, shortName, efforts
    )
  }
}

/// The catalog half of cox-ffi's `App`.
public protocol ModelsClient: Sendable {
  /// Each tier's models for a session in `cwd`, the tier's configured one first.
  func models(cwd: String) throws -> [ModelChoice]
  /// The model popover's sections for a session in `cwd`.
  func modelMenu(cwd: String) throws -> [ModelSection]
  /// The provider sections a turn in `cwd` could run on now: a key found or a local server
  /// listening. Probes the servers, so it waits.
  func usableProviders(cwd: String) async throws -> [String]
}

/// A fixed catalog and provider list: enough to drive the popover and footer in a test.
public struct FixtureModels: ModelsClient {
  public var fixedModels: [ModelChoice]
  public var fixedProviders: [String]

  public init(models: [ModelChoice] = [], providers: [String] = []) {
    (fixedModels, fixedProviders) = (models, providers)
  }

  public func models(cwd: String) -> [ModelChoice] { fixedModels }

  /// One section per tier in first-listed order; not the core's rule, which also lists a model
  /// once across tiers.
  public func modelMenu(cwd: String) -> [ModelSection] {
    var sections: [ModelSection] = []
    for choice in fixedModels {
      let model = MenuModel(
        id: choice.id, displayName: choice.displayName, shortName: choice.shortName,
        efforts: choice.efforts.map(\.rawValue).joined(separator: " · "))
      if let index = sections.firstIndex(where: { $0.tier == choice.tier }) {
        sections[index].models.append(model)
      } else {
        sections.append(
          ModelSection(
            tier: choice.tier, title: choice.tier.rawValue.capitalized, models: [model]))
      }
    }
    return sections
  }
  public func usableProviders(cwd: String) async -> [String] { fixedProviders }
}
