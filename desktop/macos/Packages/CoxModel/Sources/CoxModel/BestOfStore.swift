// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// One best-of-n group's compare view (DT§3.3.1, T52.11): its columns, read again whenever the
// app says a session started, stopped or began to wait, and the pick with its second
// confirmation for worktrees that hold changes. Here, not in the view, so the rule that changes
// are never discarded on the first yes is tested without a window.

import CoxClient
import Observation

@MainActor
@Observable
public final class BestOfStore {
  /// A pick that left worktrees with changes: "Keep this one" asks again before discarding them.
  public struct Pending: Equatable, Sendable {
    public var keep: UInt32
    public var dirty: [String]
  }

  public let id: String
  /// One per candidate, in launch order.
  public private(set) var columns: [CandidateView] = []
  /// Why the last read or pick failed; the columns keep what was read before.
  public private(set) var failure: String?
  public private(set) var pending: Pending?
  /// Why a pick left the rest: still running, or git refused.
  public private(set) var refused: [String] = []

  private let client: any BestOfClient

  public init(id: String, client: any BestOfClient) { (self.id, self.client) = (id, client) }

  public func refresh() async {
    do {
      columns = try await client.compare(id)
      failure = nil
    } catch {
      failure = String(describing: error)
    }
  }

  /// Reads the columns now and again after every change the workspace reports, until cancelled.
  public func follow(_ workspace: any WorkspaceClient) async {
    await refresh()
    while !Task.isCancelled {
      do {
        try await workspace.changed()
      } catch {
        try? await Task.sleep(for: .seconds(1))
      }
      guard !Task.isCancelled else { return }
      await refresh()
    }
  }

  /// What keeping `index` prunes, for the confirmation to list: every other candidate that still
  /// has a worktree.
  public func prunes(keeping index: UInt32) -> [CandidateView] {
    columns.filter { $0.index != index && $0.worktree != nil && $0.state != .pruned }
  }

  /// Keeps `index` and prunes the clean worktrees; ones with changes wait in `pending`.
  public func keep(_ index: UInt32) async { await pick(index, discard: false) }

  /// The second yes: discards the changes `pending` lists.
  public func discard() async {
    guard let pending else { return }
    await pick(pending.keep, discard: true)
  }

  /// The second question answered no: the worktrees with changes stay.
  public func keepChanges() { pending = nil }

  private func pick(_ index: UInt32, discard: Bool) async {
    do {
      let picked = try await client.pick(id, keep: index, discard: discard)
      pending = picked.dirty.isEmpty ? nil : Pending(keep: index, dirty: picked.dirty)
      refused = picked.refused
      failure = nil
    } catch {
      failure = String(describing: error)
    }
    await refresh()
  }
}
