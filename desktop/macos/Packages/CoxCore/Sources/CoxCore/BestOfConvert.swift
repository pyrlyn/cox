// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Best of n over cox-ffi (T52.11): `App.bestOf`, `compare` and `pick` as CoxClient's values.
// Field for field; nothing is decided here. Separate from `LiveCoreClient.swift` like the other
// conversions, so that file stays the list of calls into Rust for one session.

import CoxClient
import CoxFFIBindings

extension LiveCoreClient: BestOfClient {
  public func bestOf(
    _ request: CoxClient.BestOfRequest, theme: String
  ) async throws -> CoxClient.BestOfLaunch {
    let launch = try await app.bestOf(
      request: CoxFFIBindings.BestOfRequest(
        project: request.project, prompt: request.prompt,
        candidates: request.candidates.map { CoxFFIBindings.Candidate($0) }),
      theme: theme)
    return CoxClient.BestOfLaunch(
      id: launch.group.id, sessions: launch.sessions.map { LiveSession($0) })
  }

  public func compare(_ id: String) async throws -> [CoxClient.CandidateView] {
    try await app.compare(id: id).enumerated().map {
      CoxClient.CandidateView(UInt32($0.offset), $0.element)
    }
  }

  public func pick(_ id: String, keep: UInt32, discard: Bool) async throws -> CoxClient.Picked {
    let picked = try await app.pick(id: id, keep: keep, discard: discard)
    return CoxClient.Picked(pruned: picked.pruned, dirty: picked.dirty, refused: picked.refused)
  }
}

extension CoxFFIBindings.Candidate {
  init(_ candidate: CoxClient.Candidate) {
    switch candidate {
    case .cox(let model): self = .cox(model: model)
    case .agent(let name): self = .agent(name: name)
    }
  }
}

extension CoxClient.Candidate {
  init(_ candidate: CoxFFIBindings.Candidate) {
    switch candidate {
    case .cox(let model): self = .cox(model: model)
    case .agent(let name): self = .agent(name: name)
    }
  }
}

extension CoxClient.CandidateState {
  init(_ state: CoxFFIBindings.CandidateState) {
    switch state {
    case .running: self = .running
    case .waitingOnYou: self = .waitingOnYou
    case .done: self = .done
    case .failed(let why): self = .failed(why: why)
    case .kept: self = .kept
    case .pruned: self = .pruned
    }
  }
}

extension CoxClient.CandidateView {
  init(_ index: UInt32, _ view: CoxFFIBindings.CandidateView) {
    self.init(
      index: index, candidate: CoxClient.Candidate(view.candidate),
      state: CoxClient.CandidateState(view.state), session: view.session,
      worktree: view.worktree, branch: view.branch,
      files: view.files.map {
        CoxClient.FileStat(path: $0.path, added: $0.added, removed: $0.removed)
      },
      costUsd: view.costUsd, durationMs: view.durationMs)
    // The core's own label and totals, in case they ever differ from the sum of the rows.
    (label, added, removed) = (view.label, view.added, view.removed)
  }
}
