// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `SessionWindow`'s intent handlers: what the shell, the toolbar and the sidebar report, turned
// into pane state, store calls and core intents. Separate from `SessionWindow.swift` only to keep
// that file within SwiftLint's length limits; the members both files use are internal for it.

import CoxClient
import CoxModel
import CoxUI
import SwiftUI

extension SessionWindow {
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
    case .appearance(let change):
      screen.appearance.apply(change)
      screen.appearance.fillTexts()
      let edit = AppearanceEdit(change)
      guard let settings = model.settings else { return }
      appearanceWrites.submit(edit.key) { await settings.apply(edit) }
    }
  }

  func handle(_ intent: SessionToolbar.Intent) {
    switch intent {
    case .showSidebar: toggleSidebar()
    case .toggleInspector: screen.isInspectorVisible.toggle()
    case .open(.appearance): screen.popover = screen.popover == .appearance ? nil : .appearance
    case .mode(let mode): send(.setMode(mode: PermissionMode(mode)))
    case .stop: send(.interrupt)
    case .open(.cost):
      // DT§5.1: the cost pill opens Context & Cost.
      (screen.inspectorTab, screen.isInspectorVisible) = (.context, true)
    case .open(.model): screen.popover = screen.popover == .model ? nil : .model
    case .rename(let title): send(.rename(title: title))
    }
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

  /// A refusal shows until dismissed.
  func send(_ intent: Intent, to store: SessionStore) {
    Task {
      do {
        _ = try await store.send(intent)
      } catch {
        refused = String(describing: error)
      }
    }
  }
}
