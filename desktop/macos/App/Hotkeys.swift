// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The global hotkeys (T51.15, DT§4.6): "Show cox menu" and "New session", unbound until the
// person records one in Settings › General. KeyboardShortcuts owns the recorder, the
// registration and the storage (its own UserDefaults keys: UI state, not config), so nothing
// here reaches Rust. Apart from the scenes because the handlers fire outside every view and
// reach a window through the `openWindow` a scene handed to `AppModel`.

import AppKit
import CoxUI
import KeyboardShortcuts
import SwiftUI

extension KeyboardShortcuts.Name {
  /// No initial shortcut: a default could steal a combination another app relies on.
  static let showCoxMenu = Self("showCoxMenu")
  static let newSession = Self("newSession")
}

@MainActor
enum Hotkeys {
  /// Settings › General's rows, by the names the recorder stores them under.
  static let shortcuts: [SettingsScreen.Shortcut] = [
    .init(
      id: KeyboardShortcuts.Name.showCoxMenu.rawValue, title: "Show cox menu",
      detail: "Opens the menu-bar panel from any app."),
    .init(
      id: KeyboardShortcuts.Name.newSession.rawValue, title: "New session",
      detail: "Starts a session in a new window."),
  ]

  /// The library's recorder for one row; it stores what the person presses.
  static func recorder(_ id: SettingsScreen.Shortcut.ID) -> AnyView {
    AnyView(KeyboardShortcuts.Recorder(for: KeyboardShortcuts.Name(id)))
  }

  /// Once per launch: the library appends each handler for the life of the process.
  static func register(_ model: AppModel) {
    KeyboardShortcuts.onKeyUp(for: .newSession) { [weak model] in
      NSApp.activate()
      // A new main window opens a new session, as the menu bar's New session does.
      model?.show(id: CoxApp.mainWindow)
    }
    KeyboardShortcuts.onKeyUp(for: .showCoxMenu) { [weak model] in
      if let button = statusButton() {
        button.performClick(nil)
      } else {
        // `desktop.menu_bar` is off: the app is the next best place for what needs you.
        NSApp.activate()
        model?.show(id: CoxApp.mainWindow)
      }
    }
  }

  /// The menu-bar extra's button. SwiftUI keeps its `NSStatusItem` private, so the button is
  /// found in the app's own status-bar window; the app has no other status item.
  private static func statusButton() -> NSStatusBarButton? {
    NSApp.windows.lazy.compactMap { button(in: $0.contentView) }.first
  }

  private static func button(in view: NSView?) -> NSStatusBarButton? {
    guard let view else { return nil }
    if let button = view as? NSStatusBarButton { return button }
    return view.subviews.lazy.compactMap { button(in: $0) }.first
  }
}

extension View {
  /// Hands this scene's `openWindow` to the model, for the hotkeys and the intents, which fire
  /// outside every view.
  func lendsOpenWindow(to model: AppModel) -> some View {
    modifier(LentOpenWindow(model: model))
  }
}

private struct LentOpenWindow: ViewModifier {
  let model: AppModel
  @Environment(\.openWindow) private var openWindow

  func body(content: Content) -> some View {
    content.onAppear { model.lend(openWindow) }
  }
}
