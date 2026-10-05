// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The inspector's Info tab (T37.29.5, DT§5.1): cox-app's `Info` as the rows CoxUI's
// `InfoTab.State` holds — the session's facts and the config layers it runs with, listed as the
// core built them (T58.4.21: `~`, `detached` and the key counts are decided in `cox_app::info`).
// The app copies them into `InfoTab.State` field for field.

import CoxClient

public struct InfoTabState: Equatable, Sendable {
  /// `KeyValueGrid.Row`.
  public struct Fact: Equatable, Sendable {
    public var label: String
    public var values: [String]
    public var isDetail = false
  }

  /// Id, folder, worktree and its branch, rollout.
  public var session: [Fact] = []
  /// A row per layer with its key count, its file under it as a detail row.
  public var config: [Fact] = []

  public init() {}

  public init(_ info: Info) {
    (session, config) = (info.facts.map(Fact.init), info.configFacts.map(Fact.init))
  }
}

extension InfoTabState.Fact {
  /// A core fact as a grid row: its value alone, none for a label-only row.
  init(_ fact: CoxClient.Fact) {
    self.init(label: fact.label, values: fact.value.map { [$0] } ?? [], isDetail: fact.detail)
  }
}

extension SessionStore {
  /// The Info tab's state, read from the core when the tab asks (T37.29.5).
  public func infoTab() async throws -> InfoTabState {
    InfoTabState(try await session.info())
  }
}
