// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The View menu's pane items (DS§4, DT§5.1, A89): Show/Hide Sidebar on ⌃⌘S and Show/Hide
// Inspector on ⌃⌘I, the keys and titles of the system `SidebarCommands` and
// `InspectorCommands`. Those act only on panes the system built, and `MainScreen` lays its panes
// out itself, so these items take the system sidebar group's place and toggle the focused
// window's panes through the actions it publishes. Show/Hide Terminal on ⌃` folds the session's
// terminal pane under the column (DT§5.5, T51.6); Show/Hide Browser on ⌘⇧B puts the browser pane
// beside it (DT§5.5, T51.10). The Window menu's Open in New Window and Open in New Tab pop the
// shown session out (T51.11). File › Connect to Host… opens the focused window's sheet for an ssh
// host alias (T52.21). File › New Session (⌘N) opens a session in the focused window, New Window
// a main window of its own, Command Palette… (⌘K) the palette over it and Review Changes (⌘⇧R)
// shows or hides Review (DT§5.5, T37.44.13); they replace the system's New Window item.

import CoxUI
import SwiftUI

/// The focused session window's pane toggles and whether each pane is out.
struct ShellActions {
  var isSidebarVisible: Bool
  var isInspectorVisible: Bool
  var isTerminalVisible: Bool
  var isBrowserVisible: Bool
  var toggleSidebar: () -> Void
  var toggleInspector: () -> Void
  var toggleTerminal: () -> Void
  var toggleBrowser: () -> Void
  /// Opens the shown session in a new window, or a tab when `true`; `nil` with none shown.
  var popOut: ((Bool) -> Void)?
  /// Shows the Connect to Host sheet; `nil` for a launch that cannot connect (a fixture).
  var connectHost: (() -> Void)?
  /// Opens a new session in this window, asking for its agent first when one is configured.
  var newSession: () -> Void
  /// Shows or hides Review; `nil` with no session shown.
  var review: (() -> Void)?
  /// Opens or closes the command palette; `nil` with no session shown.
  var palette: (() -> Void)?
}

extension FocusedValues {
  @Entry var shell: ShellActions?
}

struct ShellCommands: Commands {
  @FocusedValue(\.shell) private var shell
  @Environment(\.openWindow) private var openWindow

  var body: some Commands {
    CommandGroup(replacing: .newItem) {
      // With no session window focused, a new main window opens its first session.
      Button("New Session") {
        if let shell { shell.newSession() } else { openWindow(id: CoxApp.mainWindow) }
      }
      .keyboardShortcut(ShellShortcut.newSession.key)
      Button("New Window") { openWindow(id: CoxApp.mainWindow) }
      Button("Command Palette…") { shell?.palette?() }
        .keyboardShortcut(ShellShortcut.palette.key)
        .disabled(shell?.palette == nil)
      Button("Review Changes") { shell?.review?() }
        .keyboardShortcut(ShellShortcut.review.key)
        .disabled(shell?.review == nil)
    }
    CommandGroup(after: .newItem) {
      Button("Connect to Host…") { shell?.connectHost?() }
        .disabled(shell?.connectHost == nil)
    }
    CommandGroup(replacing: .sidebar) {
      Button(shell?.isSidebarVisible == false ? "Show Sidebar" : "Hide Sidebar") {
        shell?.toggleSidebar()
      }
      .keyboardShortcut(ShellShortcut.sidebar.key)
      .disabled(shell == nil)
      Button(shell?.isInspectorVisible == false ? "Show Inspector" : "Hide Inspector") {
        shell?.toggleInspector()
      }
      .keyboardShortcut(ShellShortcut.inspector.key)
      .disabled(shell == nil)
      Button(shell?.isTerminalVisible == true ? "Hide Terminal" : "Show Terminal") {
        shell?.toggleTerminal()
      }
      .keyboardShortcut(ShellShortcut.terminal.key)
      .disabled(shell == nil)
      Button(shell?.isBrowserVisible == true ? "Hide Browser" : "Show Browser") {
        shell?.toggleBrowser()
      }
      .keyboardShortcut(ShellShortcut.browser.key)
      .disabled(shell == nil)
    }
    CommandGroup(after: .windowArrangement) {
      Button("Open in New Window") { shell?.popOut?(false) }
        .disabled(shell?.popOut == nil)
      Button("Open in New Tab") { shell?.popOut?(true) }
        .disabled(shell?.popOut == nil)
    }
  }
}
