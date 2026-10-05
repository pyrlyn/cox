// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The toolbar's model popover (DT§5.1 "model chip", T37.22.6): the core's sections (T58.4.7), each
// row by the core's short name (A111, A129), the one the session runs on marked.
// A pick is `/model <tier> <id>`, the TUI's switch. Here, not in CoxUI, because the store owns
// what the menu shows (DS§1); the app copies it into `ModelPopover.State`.

import CoxClient

public struct ModelMenu: Equatable, Sendable {
  public struct Row: Identifiable, Equatable, Sendable {
    public var id: String { "\(tier.rawValue)/\(model)" }
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
    /// `Code`, `Think`, `Cheap`.
    public let title: String
    public let rows: [Row]
  }

  public var sections: [Section] = []

  public init() {}

  /// The core's sections as they are (`App.modelMenu`), the running model marked.
  public init(sections: [ModelSection], status: Status) {
    self.sections = sections.map { section in
      Section(
        title: section.title,
        rows: section.models.map { model in
          Row(
            tier: section.tier, model: model.id,
            name: model.shortName ?? model.id, detail: model.efforts,
            isSelected: model.id == status.model)
        })
    }
  }

  /// The switch a row's click sends; `nil` for a row the menu does not list.
  public func pick(_ row: Row.ID) -> Intent? {
    sections.lazy.flatMap(\.rows).first { $0.id == row }.map {
      .switchModel(tier: $0.tier, model: $0.model)
    }
  }
}
