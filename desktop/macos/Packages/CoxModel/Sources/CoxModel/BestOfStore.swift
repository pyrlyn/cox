// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// One best-of-n group's compare view (DT§3.3.1, T52.11): its columns, read again whenever the
// app says a session started, stopped or began to wait, and the pick with its second
// confirmation for worktrees that hold changes. Here, not in the view, so the rule that changes
// are never discarded on the first yes is tested without a window.

import CoxClient
import Observation

extension CandidateView {
  /// "Open in Review" needs a session to open and something to review: a candidate that failed
  /// and changed no file has nothing in its worktree worth a diff.
  public var canReview: Bool {
    guard session != nil, state != .pruned else { return false }
    if case .failed = state { return !files.isEmpty }
    return true
  }

  /// "Keep this one": only a finished candidate that still has its worktree, while no other has
  /// been kept.
  public func canKeep(anotherKept: Bool) -> Bool {
    !anotherKept && state == .done && worktree != nil
  }
}

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

  /// The candidates a merge may take, as the core lists them (T52.25).
  public private(set) var mergeable: [UInt32] = []
  /// The candidates the person ticked for the merge; always a subset of `mergeable`.
  public private(set) var chosen: Set<UInt32> = []
  /// Who merges them; chosen from the same candidates the best-of control offers.
  public var merger: Candidate?

  private let client: any BestOfClient

  public init(id: String, client: any BestOfClient) { (self.id, self.client) = (id, client) }

  public func refresh() async {
    do {
      columns = try await client.compare(id)
      mergeable = try await client.mergeable(id)
      // A candidate that was pruned, picked over or is gone can no longer be merged.
      chosen.formIntersection(mergeable)
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

  /// Ticks or unticks candidate `index` for the merge; one the core does not list is ignored.
  public func toggle(_ index: UInt32) {
    guard mergeable.contains(index) else { return }
    if chosen.contains(index) { chosen.remove(index) } else { chosen.insert(index) }
  }

  /// The merge as the core takes it: two or more chosen candidates and a merger.
  public var mergeRequest: BestOfMerge? {
    guard chosen.count >= 2, let merger else { return nil }
    return BestOfMerge(id: id, from: Array(chosen), merger: merger)
  }

  /// Merges the chosen candidates. The columns are read again so the merge candidate shows at
  /// once; the person's choice is cleared when it started, and kept when the core refused it.
  /// Returns the launch, whose session the window opens.
  @discardableResult
  public func merge(theme: String) async -> BestOfLaunch? {
    guard let request = mergeRequest else { return nil }
    var launch: BestOfLaunch?
    do {
      launch = try await client.merge(request, theme: theme)
      chosen = []
      failure = nil
    } catch {
      failure = String(describing: error)
    }
    let reason = failure
    await refresh()
    // `refresh` clears a failure it did not hit; the refusal is still the news.
    if let reason { failure = reason }
    return launch
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
