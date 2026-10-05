// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The window shell's values from the live stores (DT§5.1, T37.22.5): the toolbar and its model
// popover from the open session's store, Info and model catalog (T37.22.6), the sidebar from
// `SidebarStore` and the providers' health, copied into CoxUI's `SessionToolbar.State`,
// `ModelPopover.State` and `Sidebar.State` field for field. Wiring only:
// CoxModel decided every text and status; separate from the window so its body stays the layout.

import CoxClient
import CoxModel
import CoxTranscript
import CoxUI
import Foundation

/// A session this window shows: the stores it shares through `AppStore` with every other window
/// on the session (their pull runs while another session shows), what it reported at open, and
/// this window's terminal tabs' views.
@MainActor
struct OpenedSession {
  let store: SessionStore
  let composer: ComposerStore
  /// What it reported after it showed; nil until then.
  var info: Info?
  /// The models its cwd's config offers, for best-of's options; empty until read.
  var models: [ModelChoice] = []
  /// The model popover's sections the core built for its cwd (T58.4.13); empty until read.
  var modelSections: [ModelSection] = []
  /// Who can drive a session in its cwd (T52.8): the Info tab's Agents list and what the UI
  /// calls the session's own agent. Empty until read.
  var agents: [AgentChoice] = []

  /// What the UI calls the external agent driving it, from its row in `cox.db`; `nil` for cox.
  func agent(in sidebar: SidebarStore) -> String? {
    sidebar.entry(store.session.id)?.session.agent.map { agents.label(of: $0) }
  }
  /// Its terminal tabs' views, kept while each tab is open (T51.6).
  let terminals = TerminalSurfaces()

  /// The toolbar's model menu for what the session runs on now.
  var menu: ModelMenu { ModelMenu(sections: modelSections, status: store.status) }

  init(_ shared: AppStore.Shared) {
    (store, composer) = (shared.store, shared.composer)
  }

  /// This window stops showing the session: its terminal views detach, and the registry closes
  /// the session once no window shows it. The core keeps a running turn either way.
  func close(in registry: AppStore, window: UUID) {
    terminals.endAll()
    registry.release(store.session.id, window: window)
  }
}

@MainActor
enum ShellState {
  static func toolbar(
    _ open: OpenedSession?, sidebar: SidebarStore, popover: SessionToolbar.Popover?
  ) -> SessionToolbar.State {
    guard let open else { return SessionToolbar.State(popover: popover) }
    let figures = ToolbarState(
      usage: open.store.usage, entry: sidebar.entry(open.store.session.id), info: open.info,
      agents: open.agents)
    return SessionToolbar.State(
      title: figures.title, project: figures.project, branch: figures.branch,
      model: figures.model ?? open.composer.model ?? "",
      mode: open.store.status.mode.map(SessionMode.init) ?? .ask,
      cost: figures.cost, context: figures.context, contextFraction: figures.contextFraction,
      isRunning: open.store.isTurnRunning, popover: popover,
      pluginStatus: PluginWidgets.status(open.store))
  }

  static func models(_ menu: ModelMenu?) -> ModelPopover.State {
    ModelPopover.State(
      sections: (menu?.sections ?? []).map { section in
        ModelPopover.Section(
          title: section.title,
          rows: section.rows.map {
            CompletionList.Row(id: $0.id, title: $0.name, detail: $0.detail)
          },
          selected: section.rows.first(where: \.isSelected)?.id)
      })
  }

  /// The New-session sheet from the picker's list and pick.
  static func picker(_ picker: AgentPicker) -> AgentPickerSheet.State {
    AgentPickerSheet.State(
      rows: picker.choices.map { AgentsList.Row($0) }, selection: picker.selection.id,
      failure: picker.failure)
  }

  /// The local list, then one group per remote host (T52.21).
  static func sidebar(
    _ store: SidebarStore, remotes: RemoteHosts, selection: String?, providers: ProviderHealth
  ) -> Sidebar.State {
    Sidebar.State(
      filter: store.filter, groups: (store.sections + remotes.sections).map(group),
      selection: selection,
      providers: providers.text, providerStatus: status(providers.status))
  }

  private static func group(_ section: CoxModel.SidebarSection) -> Sidebar.Group {
    let kind: Sidebar.Group.Kind =
      switch section.kind {
      case .section(let count): .section(count: count)
      case .project(let isExpanded): .project(isExpanded: isExpanded)
      case .host(let isConnected): .host(isConnected: isConnected)
      }
    return Sidebar.Group(
      id: section.id, title: section.title, kind: kind,
      sessions: section.rows.map { row in
        Sidebar.Session(
          id: row.id,
          row: SessionRow.Item(
            status: status(row.status), title: row.title, subtitle: row.subtitle, cost: row.cost),
          session: row.session == row.id ? nil : row.session, isReadOnly: row.isReadOnly)
      })
  }

  private static func status(_ status: CoxModel.SidebarRow.Status) -> StatusDot.Status {
    switch status {
    case .running: .running
    case .waiting: .waiting
    case .idle: .idle
    case .error: .error
    }
  }
}
