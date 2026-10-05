// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The inspector's tabs for one session (DT§5.1, T37.29, T37.22.5): Changes, Plan, Context, Tasks
// and Info read from `SessionStore`, copied into CoxUI's tab states field for field. Here, where
// CoxUI and CoxModel meet, so the app only places the view: CoxModel decided every text; the
// intents that stay in the session (revert, rewind, compact) go to the store, a shell task's output
// opens in a sheet here (T37.22.6), and the ones that change the window (Review, another session)
// go to the app.

import CoxClient
import CoxModel
import CoxUI
import SwiftUI

/// The selected tab's content. A tab the core answers by a call is read when it shows and again
/// whenever the transcript grows or a turn starts or ends; the rest follow the store.
public struct SessionInspector: View {
  /// What the window does for a tab.
  public enum Request: Equatable, Sendable {
    /// Open Review, at this file or the first changed one.
    case review(path: String?)
    /// Show this session: a subagent's transcript.
    case open(session: String)
    /// The core refused an intent; the text says why.
    case refused(String)
  }

  let store: SessionStore
  let tab: InspectorTab
  let cacheHit: CacheHitScope
  /// Who can drive a session in its cwd, for the Info tab's Agents list (T52.8).
  let agents: [AgentChoice]
  let request: @MainActor (Request) -> Void
  @State private var changes = ChangesTabState()
  @State private var costs = CostHistoryState()
  @State private var info = InfoTabState()
  @State private var plan: [TodoItem] = []
  /// The shell task whose archived output the sheet shows.
  @State private var output: Output?

  public init(
    store: SessionStore, tab: InspectorTab, cacheHit: CacheHitScope, agents: [AgentChoice] = [],
    request: @escaping @MainActor (Request) -> Void
  ) {
    (self.store, self.tab, self.cacheHit, self.request) = (store, tab, cacheHit, request)
    self.agents = agents
  }

  public var body: some View {
    content.task(id: Reload(tab: tab, blocks: store.blocks.count, running: store.isTurnRunning)) {
      await read()
    }
    .sheet(item: $output) { shown in
      TaskOutputSheet(title: shown.title, output: shown.text) { output = nil }
    }
  }

  /// A shell's output as the sheet shows it.
  private struct Output: Identifiable {
    let id: String
    let title: String
    let text: String
  }

  @ViewBuilder private var content: some View {
    switch tab {
    case .changes: ChangesTab(state: ChangesTab.State(changes), send: changed)
    case .plan: PlanTab(state: PlanTab.State(plan))
    case .context:
      ContextTab(
        state: ContextTab.State(store.contextTab(cacheHit: cacheHit), costs: costs), send: compact)
    case .tasks: TasksTab(state: TasksTab.State(store.tasks), send: opened)
    case .info: InfoTab(state: InfoTab.State(info, agents: agents))
    }
  }

  /// When a tab is read again.
  private struct Reload: Equatable {
    let tab: InspectorTab
    let blocks: Int
    let running: Bool
  }

  /// A read the core fails leaves the tab as it was; the next reload tries again.
  private func read() async {
    switch tab {
    case .changes: changes = (try? await store.changesTab()) ?? changes
    case .plan: plan = store.session.plan()
    case .context: costs = (try? await store.costHistory()) ?? costs
    case .tasks: break
    case .info: info = (try? await store.infoTab()) ?? info
    }
  }

  private func changed(_ intent: ChangesTab.Intent) {
    switch intent {
    case .review: request(.review(path: nil))
    case .open(let path): request(.review(path: path))
    case .revert(let path): perform { try await store.revert(path: path) }
    case .rewind(let checkpoint): perform { try await store.rewind(checkpoint: checkpoint) }
    }
  }

  private func compact(_ intent: ContextTab.Intent) {
    perform { try await store.compactNow() }
  }

  private func opened(_ intent: TasksTab.Intent) {
    guard case .open(let task) = intent else { return }
    do {
      switch try store.open(task: task) {
      case .transcript(let session): request(.open(session: session))
      case .output(let archive):
        let label = store.tasks.first { $0.id == task }?.label ?? task
        output = Output(
          id: archive, title: label, text: try store.session.output(archive: archive))
      case nil: break
      }
    } catch {
      request(.refused(String(describing: error)))
    }
  }

  private func perform(_ work: @escaping @MainActor () async throws -> Void) {
    Task {
      do {
        try await work()
      } catch {
        request(.refused(String(describing: error)))
      }
    }
  }
}

extension ChangesTab.State {
  init(_ tab: ChangesTabState, selection: String? = nil) {
    self.init(
      files: tab.files.map(ChangedFileRow.File.init), selection: selection,
      checkpoints: tab.checkpoints.map(CheckpointRow.Checkpoint.init),
      worktree: tab.worktree.map { KeyValueGrid.Row(label: $0.label, values: $0.values) })
  }
}

extension ChangedFileRow.File {
  init(_ file: ChangesTabState.File) {
    let change: ChangedFileRow.Change =
      switch file.change {
      case .edited: .edited
      case .created: .created
      case .deleted: .deleted
      }
    self.init(path: file.path, change: change, added: file.added, removed: file.removed)
  }
}

extension CheckpointRow.Checkpoint {
  init(_ checkpoint: ChangesTabState.Checkpoint) {
    self.init(id: checkpoint.id, label: checkpoint.label, time: checkpoint.time)
  }
}

extension PlanTab.State {
  init(_ plan: [TodoItem]) {
    self.init(
      items: plan.map { item in
        let step: PlanTab.Step =
          switch item.state {
          case .pending: .pending
          case .inProgress: .inProgress
          case .done: .done
          }
        return PlanTab.Item(id: item.id, text: item.text, step: step)
      })
  }
}

extension ContextTab.State {
  init(_ tab: ContextTabState, costs: CostHistoryState) {
    self.init(
      context: tab.split.context, share: tab.split.share,
      parts: tab.split.parts.compactMap { part in
        StackedBar.Kind(rawValue: part.kind.rawValue).map {
          ContextTab.Part(kind: $0, fraction: part.fraction, label: part.label, tokens: part.tokens)
        }
      },
      free: tab.split.free, cacheHit: tab.cacheHit, turnRunning: tab.turnRunning,
      costColumns: costs.columns,
      costs: costs.rows.map {
        KeyValueGrid.Row(label: $0.label, values: $0.values, isDetail: $0.isDetail)
      },
      footnote: costs.footnote)
  }
}

extension TasksTab.State {
  init(_ rows: [TaskRow]) {
    self.init(
      items: rows.map { row in
        let state: ToolHeader.State =
          switch row.state {
          case .running: .running
          case .succeeded: .succeeded
          case .failed: .failed
          }
        return TasksTab.Item(
          id: row.id, label: row.label, tier: row.tier, kind: row.kind == .agent ? .agent : .shell,
          state: state, cost: row.cost)
      })
  }
}

extension InfoTab.State {
  init(_ tab: InfoTabState, agents: [AgentChoice] = []) {
    self.init(
      session: tab.session.map {
        KeyValueGrid.Row(label: $0.label, values: $0.values, isDetail: $0.isDetail)
      },
      config: tab.config.map {
        KeyValueGrid.Row(label: $0.label, values: $0.values, isDetail: $0.isDetail)
      },
      agents: agents.map { AgentsList.Row($0) })
  }
}

extension AgentsList.Row {
  /// One agent as the Info tab and the New-session sheet list it.
  public init(_ agent: AgentChoice) {
    self.init(
      id: agent.id, label: agent.label, origin: agent.origin, launch: agent.launch,
      unavailable: agent.unavailable)
  }
}
