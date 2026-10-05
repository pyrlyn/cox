// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Best of n in a window (DT§3.3.1, mockup 27, T52.12): the composer's control, shown while the
// composer holds a prompt, sends it to the session's own agent and every candidate added beside
// it, each in a worktree of its own; the compare sheet then follows the group and keeps one.
// Wiring only — the core launches and prunes, CoxModel's `BestOfStore` reads and picks, CoxUI
// draws. Separate from `SessionWindow` so the window's body stays the layout.

import CoxClient
import CoxModel
import CoxUI
import Foundation
import SwiftUI

/// One window's best-of-n: the candidates added, the launch under way, and the group the compare
/// sheet shows.
@MainActor
@Observable
final class BestOfLauncher {
  private(set) var picked: Set<String> = []
  private(set) var isLaunching = false
  private(set) var failure: String?
  /// The group the compare sheet shows.
  var comparing: Comparing?

  struct Comparing {
    let store: BestOfStore
    /// What the group was sent, for the sheet's heading.
    let prompt: String
  }

  func toggle(_ id: String) {
    if picked.remove(id) == nil { picked.insert(id) }
    failure = nil
  }

  /// Every other agent this cwd can run, cox if an agent drives the session, and cox on each
  /// code-tier model; `driver` is the session's own agent, `nil` for cox.
  func state(_ open: OpenedSession, driver: String?) -> BestOfControl.State {
    let agents = open.agents.compactMap { agent -> BestOfControl.Option? in
      guard let name = agent.name, name != driver else { return nil }
      return BestOfControl.Option(
        id: "agent:\(name)", label: agent.label, unavailable: agent.unavailable)
    }
    let cox = driver == nil ? [] : [BestOfControl.Option(id: "cox", label: "cox")]
    let models = open.models.filter { $0.tier == .code }.map {
      BestOfControl.Option(id: "cox:\($0.id)", label: "cox · \($0.displayName ?? $0.id)")
    }
    let options = (agents + cox + models).map { option in
      var option = option
      option.isPicked = picked.contains(option.id)
      return option
    }
    return BestOfControl.State(options: options, isLaunching: isLaunching, failure: failure)
  }

  /// Sends `prompt` to the session's own agent and the candidates added; the launch's sessions,
  /// or `nil` when it failed, which the control then says.
  func launch(
    prompt: String, project: String, driver: String?, options: [BestOfControl.Option],
    client: any BestOfClient
  ) async -> BestOfLaunch? {
    let added = options.filter(\.isPicked).map { Self.candidate($0.id) }
    let own: Candidate = driver.map { .agent(name: $0) } ?? .cox(model: nil)
    isLaunching = true
    defer { isLaunching = false }
    do {
      let launch = try await client.bestOf(
        BestOfRequest(project: project, prompt: prompt, candidates: [own] + added),
        theme: SessionWindow.syntaxTheme)
      (picked, failure) = ([], nil)
      comparing = Comparing(store: BestOfStore(id: launch.id, client: client), prompt: prompt)
      return launch
    } catch {
      failure = String(describing: error)
      return nil
    }
  }

  /// An option's id as the candidate the core launches.
  static func candidate(_ id: String) -> Candidate {
    if id.hasPrefix("agent:") { return .agent(name: String(id.dropFirst("agent:".count))) }
    return .cox(model: id.hasPrefix("cox:") ? String(id.dropFirst("cox:".count)) : nil)
  }
}

/// The composer's best-of-n control, offered while the composer holds a prompt for a local
/// session; `adopt` joins the launched sessions to the window.
struct BestOfBar: View {
  let launcher: BestOfLauncher
  let open: OpenedSession
  let model: AppModel
  let adopt: ([any SessionClient]) -> Void

  var body: some View {
    let id = open.store.session.id
    let driver = model.sidebar.entry(id)?.session.agent
    let state = launcher.state(open, driver: driver)
    if !state.options.isEmpty, let live = client(for: id) {
      BestOfControl(state: state) { intent in
        switch intent {
        case .toggle(let option): launcher.toggle(option)
        case .launch: Task { await launch(driver: driver, state: state, client: live) }
        }
      }
      .frame(maxWidth: Size.readingWidth)
      .padding(.horizontal, Space.xl)
    }
  }

  /// The live core, while the composer holds a prompt and the session is not a remote host's.
  private func client(for session: String) -> (any BestOfClient)? {
    guard !open.composer.text.isEmpty, model.remotes.workspace(for: session) == nil else {
      return nil
    }
    return try? model.launch.live.get()
  }

  /// Sends the composer's prompt from the session's cwd, then clears the composer.
  private func launch(driver: String?, state: BestOfControl.State, client: any BestOfClient) async {
    let cwd = model.sidebar.entry(open.store.session.id)?.session.cwd ?? LaunchCore.project()
    let launched = await launcher.launch(
      prompt: open.composer.text, project: cwd, driver: driver, options: state.options,
      client: client)
    guard let launched else { return }
    open.composer.edit("")
    adopt(launched.sessions)
    model.sidebar.refresh()
  }
}

/// The compare sheet over one group: it reads the columns again whenever the workspace changes,
/// and "Keep this one" asks before it prunes.
struct BestOfSheet: View {
  let store: BestOfStore
  let prompt: String
  let workspace: (any WorkspaceClient)?
  let review: (String) -> Void
  let close: () -> Void
  /// The column whose "Keep this one" waits on the first yes.
  @State private var asking: UInt32?

  var body: some View {
    BestOfCompare(state: BestOfSheet.state(store, prompt: prompt, asking: asking)) { intent in
      switch intent {
      case .review(let index):
        if let session = store.columns.first(where: { $0.index == index })?.session {
          review(session)
        }
      case .keep(let index): asking = index
      case .confirm:
        guard let keep = asking else { return }
        asking = nil
        Task { await store.keep(keep) }
      case .discard: Task { await store.discard() }
      case .cancel:
        asking = nil
        store.keepChanges()
      case .close: close()
      }
    }
    .task {
      if let workspace { await store.follow(workspace) } else { await store.refresh() }
    }
  }

  static func state(_ store: BestOfStore, prompt: String, asking: UInt32?) -> BestOfCompare.State {
    let isKept = store.columns.contains { $0.state == .kept }
    let columns = store.columns.map { view in
      BestOfCompare.Column(
        id: view.index, label: view.label, status: status(view.state), branch: view.branch,
        files: view.files.map {
          BestOfCompare.File(path: $0.path, added: Int($0.added), removed: Int($0.removed))
        },
        cost: cost(view), duration: duration(view.durationMs),
        canReview: view.session != nil && view.state != .pruned,
        canKeep: !isKept && view.state == .done && view.worktree != nil)
    }
    var confirm: BestOfCompare.Confirm?
    if let pending = store.pending {
      confirm = BestOfCompare.Confirm(
        keep: pending.keep, label: label(store, pending.keep), prunes: [], dirty: pending.dirty)
    } else if let asking {
      confirm = BestOfCompare.Confirm(
        keep: asking, label: label(store, asking),
        prunes: store.prunes(keeping: asking).map {
          "\($0.label) · \($0.branch ?? $0.worktree ?? "")"
        })
    }
    let total = store.columns.reduce(0) { $0 + $1.costUsd }
    return BestOfCompare.State(
      prompt: prompt, total: String(format: "$%.2f", total), columns: columns, confirm: confirm,
      notes: [store.failure].compactMap(\.self) + store.refused)
  }

  private static func label(_ store: BestOfStore, _ index: UInt32) -> String {
    store.columns.first { $0.index == index }?.label ?? ""
  }

  private static func status(_ state: CandidateState) -> BestOfCompare.Status {
    switch state {
    case .running: .running
    case .waitingOnYou: .waitingOnYou
    case .done: .done
    case .failed(let why): .failed(why)
    case .kept: .kept
    case .pruned: .pruned
    }
  }

  /// An external agent bills itself: no cox ledger cost to show.
  private static func cost(_ view: CandidateView) -> String {
    if case .agent = view.candidate { return "—" }
    return String(format: "$%.2f", view.costUsd)
  }

  private static func duration(_ milliseconds: UInt64) -> String {
    let (minutes, seconds) = (milliseconds / 60_000, milliseconds / 1_000 % 60)
    guard minutes > 0 else { return "\(seconds)s" }
    return "\(minutes)m \(seconds < 10 ? "0" : "")\(seconds)s"
  }
}

extension View {
  /// Shows the compare sheet while `launcher` has a group to compare.
  func bestOfSheet(
    _ launcher: BestOfLauncher, workspace: (any WorkspaceClient)?,
    review: @escaping (String) -> Void
  ) -> some View {
    sheet(
      isPresented: Binding(
        get: { launcher.comparing != nil }, set: { if !$0 { launcher.comparing = nil } })
    ) {
      if let comparing = launcher.comparing {
        BestOfSheet(
          store: comparing.store, prompt: comparing.prompt, workspace: workspace,
          review: { session in
            launcher.comparing = nil
            review(session)
          },
          close: { launcher.comparing = nil })
      }
    }
  }
}
