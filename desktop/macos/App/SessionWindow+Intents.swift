// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `SessionWindow`'s intent handlers: what the shell, the toolbar and the sidebar report, turned
// into pane state, store calls and core intents. Separate from `SessionWindow.swift` only to keep
// that file within SwiftLint's length limits; the members both files use are internal for it.

import CoxClient
import CoxModel
import CoxPlatform
import CoxTranscript
import CoxUI
import SwiftUI

extension SessionWindow {
  var isLocked: Binding<Bool> {
    Binding(get: { lockedMessage != nil }, set: { if !$0 { lockedMessage = nil } })
  }

  func readSettings() async {
    appearanceWrites.onIdle = { readAppearance() }
    await model.settings?.load()
    readAppearance()
  }

  func readAppearance() {
    guard let settings = model.settings else { return }
    screen.appearance = AppearancePopover.State(settings)
  }

  /// What the shell reports: the panes fold here, and an appearance change shows at once and is
  /// written once the control rests.
  func handle(_ intent: MainScreenIntent) {
    switch intent {
    case .sidebar(let intent): handle(intent)
    case .toolbar(let intent): handle(intent)
    case .dismissPopover: screen.popover = nil
    case .inspectorTab(let tab): screen.inspectorTab = tab
    case .model(let row):
      screen.popover = nil
      if let intent = showing?.menu.pick(row) { send(intent) }
    case .addKey:
      screen.popover = nil
      openProviders()
    case .appearance(let change):
      screen.appearance.apply(change)
      screen.appearance.fillTexts()
      let edit = AppearanceEdit(change)
      guard let settings = model.settings else { return }
      appearanceWrites.submit(edit.key) { await settings.apply(edit) }
    }
  }

  /// The composer under the transcript: its model chip names an external agent's session's agent
  /// and opens the model popover over itself (T60.6); its notice's button opens Settings at Providers
  /// (T60.5).
  func composer(for showing: OpenedSession) -> SessionComposer {
    SessionComposer(
      store: showing.composer, modelLabel: ShellState.agentModel(showing, sidebar: model.sidebar),
      openModel: { screen.popover = screen.popover == .model ? nil : .model },
      openProviders: { openProviders() })
  }

  func handle(_ intent: SessionToolbar.Intent) {
    switch intent {
    case .showSidebar: toggleSidebar()
    case .toggleInspector: screen.isInspectorVisible.toggle()
    case .open(.appearance): screen.popover = screen.popover == .appearance ? nil : .appearance
    case .stop: send(.interrupt)
    case .open(.cost):
      // DT§5.1: the cost pill opens Context & Cost.
      (screen.inspectorTab, screen.isInspectorVisible) = (.context, true)
    case .open(.model): screen.popover = screen.popover == .model ? nil : .model
    case .rename(let title): send(.rename(title: title))
    case .pluginAction(let id):
      guard let store = showing?.store else { return }
      Task { _ = await store.pluginAction(id) }
    }
  }

  /// The inspector shows `tab`: a plugin's tab renders for the inspector's width while it is the
  /// one selected, and every other tab stops it (PL§15.4). The inspector scrolls, so the height
  /// offered is the screen's, the most it could show.
  func selectPluginTab(_ tab: InspectorTab) {
    guard let store = showing?.store else { return }
    guard case .plugin(let plugin) = tab else {
      store.selectPluginTab(nil, width: 0, height: 0)
      return
    }
    let size = CGSize(
      width: Size.inspectorWidth - Space.xl * 2, height: NSScreen.main?.visibleFrame.height ?? 0)
    let cells = PluginWidgets.cells(size, scale: base.textScale)
    store.selectPluginTab(plugin, width: cells.width, height: cells.height)
  }

  /// PL§15.4's window side: the selected tab tells the core which plugin tab renders, a plugin's
  /// `OpenInspector` shows its tab, a link click goes to `openPluginLink`, and focus tells the
  /// host which session is in front.
  func withPlugins(_ content: some View) -> some View {
    content
      .onChange(of: screen.inspectorTab) { _, tab in selectPluginTab(tab) }
      .onChange(of: showing?.store.shownInspector) { _, plugin in
        if let plugin { (screen.inspectorTab, screen.isInspectorVisible) = (.plugin(plugin), true) }
      }
      .environment(\.pluginLinkAction) { openPluginLink($0) }
      .onChange(of: current, initial: true) { old, _ in markFront(replacing: old) }
      .onChange(of: activeState) { markFront(replacing: nil) }
      .onDisappear { current.map(FrontSession.shared.hide) }
  }

  /// Tells the host which session is in front, so a plugin's alert for it stays a notice rather
  /// than a system notification (PL§15.4).
  func markFront(replacing old: String?) {
    if let old { FrontSession.shared.hide(old) }
    guard let current else { return }
    if activeState == .key {
      FrontSession.shared.show(current)
    } else {
      FrontSession.shared.hide(current)
    }
  }

  /// A plugin link's click (PL§15.4): cox-app opens a web link through the host, and hands back a
  /// path it confined to a local session's workspace, which Finder reveals rather than opens, so
  /// a click never runs a file. A remote session's path names that host's disk, not this one's.
  func openPluginLink(_ link: String) {
    guard let showing, case .file(let path, _) = showing.store.openLink(link),
      model.remotes.workspace(for: showing.store.session.id) == nil
    else { return }
    NSWorkspace.shared.activateFileViewerSelecting([URL(filePath: path)])
  }

  func handle(_ intent: Sidebar.Intent) {
    switch intent {
    case .hide: toggleSidebar()
    case .filter(let text): model.sidebar.filter = text
    case .toggle(let project): model.sidebar.toggle(project)
    case .newSession: Task { await newSession() }
    case .popOut(let session, let asTab): openPopOut(session, asTab: asTab)
    case .reconnect(let group): reconnect(group)
    case .rename(let session, let title): rename(session, to: title)
    case .select(let session):
      reviewing = nil
      if opened[session] != nil {
        current = session
      } else if !model.launch.isFixture {
        // A recording replays one session; its inbox rows name sessions it cannot open.
        Task { await open(resume: session) }
      }
    }
  }

  /// A popped-out window has no sidebar to show.
  func toggleSidebar() {
    if popOut == nil { screen.isSidebarVisible.toggle() }
  }

  /// Opens `session` in a window of its own, or as a tab of this one.
  func openPopOut(_ session: String, asTab: Bool) {
    openWindow(value: PopOut(session: session, asTab: asTab))
  }

  /// ⌃`: shows or hides the terminal pane; showing it with no terminal open opens the session's
  /// shell first.
  func toggleTerminal() {
    guard let store = showing?.store else { return }
    let isShown = isTerminalShown
    if !isShown && store.terminals.isEmpty {
      do {
        try store.openTerminal()
      } catch {
        refused = String(describing: error)
        return
      }
    }
    isTerminalVisible = !isShown
  }

  /// A host group's Reconnect (T52.21).
  func reconnect(_ group: String) {
    guard let host = RemoteHosts.host(section: group) else { return }
    Task { await model.remotes.reconnect(host) }
  }

  /// An open session renames through its core; the store takes a closed one's directly.
  func rename(_ session: String, to title: String) {
    if let store = opened[session]?.store {
      send(.rename(title: title), to: store)
    } else {
      model.sidebar.rename(session, to: title)
    }
  }

  /// Sends to the session the window shows.
  func send(_ intent: Intent) {
    guard let store = showing?.store else { return }
    send(intent, to: store)
  }

  /// A refusal shows until dismissed; a provider switch puts the session it reopened in the
  /// window's place, and one the core locked offers a new session.
  func send(_ intent: Intent, to store: SessionStore) {
    Task {
      do {
        if let shared = try await model.registry.send(intent, to: store) {
          await swap(store.session.id, to: shared)
        }
      } catch let locked as ProviderLocked {
        lockedMessage = locked.message
      } catch {
        refused = String(describing: error)
      }
    }
  }

  /// A provider switch reopened `session` under the same id (DT§5.3): `AppStore` already gave
  /// its windows the new stores, their slot and the draft; this window's `OpenedSession` is
  /// rebuilt over them, and the registry entry that notifications and the menu bar answer through
  /// points at them.
  private func swap(_ session: String, to shared: AppStore.Shared) async {
    model.register(shared.store, as: session)
    guard let old = opened[session] else { return }
    opened[session] = old.reopened(as: shared)
    // The new composer reads its own readiness as it shows; the popover's marks and the footer
    // follow the same probe.
    await refreshProviders()
  }
}
