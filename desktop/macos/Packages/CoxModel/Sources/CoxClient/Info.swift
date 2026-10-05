// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// What the inspector's Info tab lists (T37.29.5, DT§5.1), field for field as cox-ffi exports
// `cox_app::Info`: the session's id, cwd, linked worktree, the config layers it runs with and its
// rollout file. Separate from the timeline because it answers a call, not a patch.

public struct Info: Equatable, Sendable {
  public var session: String
  public var cwd: String
  /// `nil` when the session runs outside a linked worktree.
  public var worktree: Linked?
  /// Each layer that set at least one key, in load order.
  public var config: [ConfigSource]
  /// The session's JSONL rollout.
  public var rollout: String
  /// Id, folder, worktree and its branch, rollout, as the tab lists them (T58.4.20).
  public var facts: [Fact]
  /// A row per layer with its key count, its file under it as a detail row.
  public var configFacts: [Fact]

  public init(
    session: String = "", cwd: String = "", worktree: Linked? = nil, config: [ConfigSource] = [],
    rollout: String = "", facts: [Fact] = [], configFacts: [Fact] = []
  ) {
    (self.session, self.cwd, self.worktree, self.config, self.rollout) = (
      session, cwd, worktree, config, rollout
    )
    (self.facts, self.configFacts) = (facts, configFacts)
  }
}

/// One row of an inspector tab's fact list, in the order it is shown (`cox_app::Fact`).
public struct Fact: Equatable, Sendable {
  public var label: String
  /// `nil` for a row that is only a label (a layer's file).
  public var value: String?
  /// Indented under the row above it.
  public var detail: Bool

  public init(label: String, value: String? = nil, detail: Bool = false) {
    (self.label, self.value, self.detail) = (label, value, detail)
  }
}

/// One config layer the session's config came from.
public struct ConfigSource: Equatable, Sendable {
  public var layer: Layer
  /// The file it was read from; `nil` for a layer that is not a file.
  public var file: String?
  /// How many effective leaves it set.
  public var keys: UInt32

  public init(layer: Layer, file: String? = nil, keys: UInt32) {
    (self.layer, self.file, self.keys) = (layer, file, keys)
  }
}
