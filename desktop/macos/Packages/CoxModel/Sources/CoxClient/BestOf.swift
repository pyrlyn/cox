// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Best of n (DT§3.3.1, T52.9–T52.11): one prompt sent to several candidates, each in a worktree
// of its own, then compared and one kept. Field for field as cox-ffi exports `cox_app::best_of`.
// Separate from the session seam because a group spans several sessions and owns none of them.

import Synchronization

/// `cox_app::Candidate`: who answers the prompt.
public enum Candidate: Hashable, Sendable {
  /// Cox itself on `model`; `nil` keeps the config's.
  case cox(model: String?)
  /// An external ACP agent by its config or plugin name.
  case agent(name: String)

  /// The compare view's column heading, as `cox_app::Candidate::label` words it.
  public var label: String {
    switch self {
    case .cox(nil): "cox"
    case .cox(let model?): "cox · \(model)"
    case .agent(let name): name
    }
  }
}

/// `cox_app::BestOfRequest`: what the composer's "Best of n" sends.
public struct BestOfRequest: Equatable, Sendable {
  /// The repository every worktree is cut from.
  public var project: String
  public var prompt: String
  public var candidates: [Candidate]

  public init(project: String, prompt: String, candidates: [Candidate]) {
    (self.project, self.prompt, self.candidates) = (project, prompt, candidates)
  }
}

/// `cox_app::CandidateState`: where a candidate is.
public enum CandidateState: Equatable, Sendable {
  case running, waitingOnYou, done
  /// It did not start, or its turn failed.
  case failed(why: String)
  case kept, pruned
}

/// `cox_protocol::traits::FileStat`: one file a candidate's worktree changed.
public struct FileStat: Equatable, Sendable {
  /// Relative to the worktree.
  public var path: String
  public var added: UInt32
  public var removed: UInt32

  public init(path: String, added: UInt32, removed: UInt32) {
    (self.path, self.added, self.removed) = (path, added, removed)
  }
}

/// `cox_app::CandidateView`: one column of the compare view. `index` is its place in the
/// group, which a pick names.
public struct CandidateView: Equatable, Sendable, Identifiable {
  public var index: UInt32
  public var candidate: Candidate
  public var label: String
  public var state: CandidateState
  public var session: String?
  public var worktree: String?
  public var branch: String?
  public var files: [FileStat]
  public var added: UInt32
  public var removed: UInt32
  /// Its own ledger rows; 0 for an external agent, which bills itself.
  public var costUsd: Double
  public var durationMs: UInt64

  public var id: UInt32 { index }

  public init(
    index: UInt32, candidate: Candidate, state: CandidateState, session: String? = nil,
    worktree: String? = nil, branch: String? = nil, files: [FileStat] = [], costUsd: Double = 0,
    durationMs: UInt64 = 0
  ) {
    (self.index, self.candidate, self.label, self.state) =
      (index, candidate, candidate.label, state)
    (self.session, self.worktree, self.branch, self.files) = (session, worktree, branch, files)
    (self.added, self.removed) = (
      files.reduce(0) { $0 + $1.added }, files.reduce(0) { $0 + $1.removed }
    )
    (self.costUsd, self.durationMs) = (costUsd, durationMs)
  }
}

/// `cox_app::Picked`: what a pick did.
public struct Picked: Equatable, Sendable {
  /// The worktrees removed.
  public var pruned: [String]
  /// The worktrees left because they hold changes; a second, confirmed pick removes them.
  public var dirty: [String]
  /// Why the rest were left: still running, or git refused.
  public var refused: [String]

  public init(pruned: [String] = [], dirty: [String] = [], refused: [String] = []) {
    (self.pruned, self.dirty, self.refused) = (pruned, dirty, refused)
  }
}

/// `cox_ffi::BestOfLaunch`: the group's id and the sessions that started, in candidate order.
public struct BestOfLaunch: Sendable {
  public var id: String
  public var sessions: [any SessionClient]

  public init(id: String, sessions: [any SessionClient]) {
    (self.id, self.sessions) = (id, sessions)
  }
}

/// The best-of-n half of cox-ffi's `App`.
public protocol BestOfClient: Sendable {
  /// One worktree and one session per candidate, each sent the prompt; a candidate that cannot
  /// start is listed with why and the others run.
  func bestOf(_ request: BestOfRequest, theme: String) async throws -> BestOfLaunch
  /// Group `id`'s columns, in candidate order.
  func compare(_ id: String) async throws -> [CandidateView]
  /// Keeps candidate `keep` and prunes the others; `discard` is the second confirmation for
  /// worktrees with changes, which are otherwise left and listed.
  func pick(_ id: String, keep: UInt32, discard: Bool) async throws -> Picked
}

/// Fixed columns and the core's pick rule over them: enough to drive the compare view in a test
/// or a preview. A worktree in `dirty` holds changes.
public final class FixtureBestOf: BestOfClient {
  private let columns: Mutex<[CandidateView]>
  private let dirty: Set<String>

  public init(_ columns: [CandidateView], dirty: Set<String> = []) {
    self.columns = Mutex(columns)
    self.dirty = dirty
  }

  /// Replaces the columns, as a candidate's progress would.
  public func set(_ columns: [CandidateView]) { self.columns.withLock { $0 = columns } }

  public func bestOf(_ request: BestOfRequest, theme: String) async -> BestOfLaunch {
    BestOfLaunch(id: "fixture", sessions: [])
  }

  public func compare(_ id: String) async -> [CandidateView] { columns.withLock { $0 } }

  public func pick(_ id: String, keep: UInt32, discard: Bool) async -> Picked {
    columns.withLock { views in
      var picked = Picked()
      for slot in views.indices {
        if views[slot].index == keep {
          views[slot].state = .kept
          continue
        }
        guard views[slot].state != .pruned, let tree = views[slot].worktree else { continue }
        if !discard && dirty.contains(tree) {
          picked.dirty.append(tree)
          continue
        }
        views[slot].state = .pruned
        picked.pruned.append(tree)
      }
      return picked
    }
  }
}
