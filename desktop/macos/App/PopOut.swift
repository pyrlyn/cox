// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A session popped out of a window (T51.11, DT§4.5): the `WindowGroup(for:)` value that opens
// one session in its own window, and the probe that makes that window a native tab of the window
// it came from. Separate from the session window because this is AppKit's tabbing, not the
// window's layout.

import AppKit
import SwiftUI

/// What a pop-out window shows. `tabOf` is the window it joins as a tab, by window number;
/// `nil` opens a window of its own. Codable, so the system restores it with the window.
struct PopOut: Codable, Hashable {
  var session: String
  var tabOf: Int?
  /// The registry token an "Ask cox" intent held the session under until this window joins it
  /// (T51.17); a restored window finds it gone, and releasing it again is a no-op.
  var handoff: UUID?

  /// For the key window's Open in New Tab, or a plain Open in New Window.
  @MainActor
  init(session: String, asTab: Bool) {
    self.session = session
    tabOf = asTab ? NSApp.keyWindow?.windowNumber : nil
  }

  /// A session an intent opened and holds under `handoff`.
  init(session: String, handoff: UUID) {
    (self.session, self.handoff) = (session, handoff)
  }
}

extension View {
  /// Joins this view's window to the tabs of window `number`, once, when it first shows.
  func joinsTabs(of number: Int?) -> some View {
    background(TabJoiner(host: number))
  }
}

private struct TabJoiner: NSViewRepresentable {
  let host: Int?

  func makeNSView(context: Context) -> Probe { Probe(host: host) }
  func updateNSView(_ probe: Probe, context: Context) {}

  final class Probe: NSView {
    let host: Int?
    private var hasJoined = false

    init(host: Int?) {
      self.host = host
      super.init(frame: .zero)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    override func viewDidMoveToWindow() {
      super.viewDidMoveToWindow()
      guard !hasJoined, let window, let host,
        let tabs = NSApp.window(withWindowNumber: host), tabs !== window
      else { return }
      hasJoined = true
      // On the next turn: the window is still being put on screen during this call.
      Task { @MainActor in tabs.addTabbedWindow(window, ordered: .above) }
    }
  }
}
