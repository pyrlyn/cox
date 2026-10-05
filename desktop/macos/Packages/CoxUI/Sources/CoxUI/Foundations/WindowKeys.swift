// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `WindowKeys` (T37.27.5, T37.27.8): the one app-local key monitor a view installs for the
// window it stands in — `DecisionBar`'s ⌘⏎ and ⌘⌫, and the composer's ⌘V of files and images.
// In its own file because both organisms need the same monitor, and two copies drifted apart.

import AppKit
import SwiftUI

/// Keys this view's window receives, seen before its first responder and the menus: an
/// app-local event monitor, because the focused editor and the Edit menu claim ⌘⏎, ⌘⌫ and ⌘V
/// before a view's own key handler or shortcut would see them. Keys for other windows go on
/// untouched. `handle` says whether it took the key; `nil` lets every key go on.
struct WindowKeys: NSViewRepresentable {
  let handle: ((NSEvent) -> Bool)?

  func makeNSView(context: Context) -> Monitor { Monitor() }

  func updateNSView(_ view: Monitor, context: Context) { view.handle = handle }

  /// `modifiers` alone of ⌘ ⇧ ⌥ ⌃ is held; Caps Lock, Fn and the keypad flag do not count.
  nonisolated static func holds(_ event: NSEvent, only modifiers: NSEvent.ModifierFlags) -> Bool {
    event.modifierFlags.intersection([.command, .shift, .option, .control]) == modifiers
  }

  final class Monitor: NSView {
    var handle: ((NSEvent) -> Bool)?
    private var monitor: Any?

    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func viewDidMoveToWindow() {
      super.viewDidMoveToWindow()
      stop()
      guard window != nil else { return }
      monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
        guard let self, event.window === self.window, self.handle?(event) == true else {
          return event
        }
        return nil
      }
    }

    isolated deinit { stop() }

    private func stop() {
      if let monitor { NSEvent.removeMonitor(monitor) }
      monitor = nil
    }
  }
}
