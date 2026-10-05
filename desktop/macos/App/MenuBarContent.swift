// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The menu-bar extra's window (T51.14, mockup 26, DT§5.6): CoxUI's `MenuBarPanel` fed by
// CoxModel's `MenuBarState` over the inbox, the sidebar's running sessions and Rust's "Today"
// figures. Allow and Deny take the notifications' route (`NotificationActions.approval`), so a
// menu click and a notification action answer an approval the same way. A question or a
// running session opens in a window of its own. Wiring only.

import AppKit
import CoxCore
import CoxModel
import CoxPlatform
import CoxUI
import SwiftUI

struct MenuBarContent: View {
  let model: AppModel
  @Environment(\.openWindow) private var openWindow
  /// `$4.02 · 7 sessions`, read each time the panel opens.
  @State private var today = ""

  var body: some View {
    MenuBarPanel(state: panel, send: handle)
      .onAppear {
        model.sidebar.refresh()
        today = (try? model.launch.live.get().today()) ?? ""
      }
  }

  private var menu: MenuBarState {
    let running = model.sidebar.sections.first { $0.id == "running" }?.rows ?? []
    return MenuBarState(inbox: model.sidebar.inboxItems, running: running, today: today) {
      model.sidebar.entry($0)?.session.title
    }
  }

  private var panel: MenuBarPanel.State {
    let menu = menu
    return MenuBarPanel.State(
      needs: menu.needs.map { need in
        let kind: MenuBarPanel.NeedKind =
          switch need.kind {
          case .approval(let command): .approval(command: command)
          case .question(let question): .question(question)
          }
        return MenuBarPanel.Need(id: need.id, session: need.session, title: need.title, kind: kind)
      },
      running: menu.running.map {
        MenuBarPanel.Running(
          id: $0.id, title: $0.title, activity: $0.activity, elapsed: "", cost: $0.cost)
      },
      today: menu.today)
  }

  private func handle(_ intent: MenuBarPanel.Intent) {
    switch intent {
    case .allow(let id): answer(id, allow: true)
    case .deny(let id): answer(id, allow: false)
    case .open(let session):
      NSApp.activate()
      openWindow(value: PopOut(session: session, asTab: false))
    case .newSession:
      NSApp.activate()
      // A new main window opens a new session.
      openWindow(id: CoxApp.mainWindow)
    case .openApp:
      NSApp.activate()
      if !NSApp.windows.contains(where: { $0.isVisible && $0.canBecomeMain }) {
        openWindow(id: CoxApp.mainWindow)
      }
    }
  }

  private func answer(_ id: MenuBarPanel.Need.ID, allow: Bool) {
    guard let need = menu.needs.first(where: { $0.id == id }),
      let route = NotificationActions.approval(
        session: need.session, call: need.call, allow: allow)
    else { return }
    model.route(route)
  }
}
