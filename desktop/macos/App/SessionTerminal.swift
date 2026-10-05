// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The session window's terminal pane (T51.6, mockup 24, DT§3.2): CoxUI's `TerminalPaneChrome`
// around CoxPlatform's SwiftTerm `TerminalPane`, fed by the session store's terminal tabs, and
// the window's close guard that asks before a foreground job in one of them is killed. Wiring
// only: the shells run in Rust under the session's sandbox (`cox_app::terminal`), CoxModel
// keeps the tabs and the packages draw.

import AppKit
import CoxClient
import CoxModel
import CoxPlatform
import CoxUI
import SwiftUI

/// Each tab's SwiftTerm view and feed, kept while the tab is open: the view holds the
/// scrollback, so a tab switch or a hidden pane must not drop it.
@MainActor
final class TerminalSurfaces {
  private var surfaces: [TerminalTab.ID: TerminalSurface] = [:]

  func surface(for tab: TerminalTab, style: TerminalStyle) -> TerminalSurface {
    if let surface = surfaces[tab.id] { return surface }
    let surface = TerminalSurface(client: tab.client, style: style, openLink: Self.open)
    surfaces[tab.id] = surface
    return surface
  }

  func end(_ id: TerminalTab.ID) { surfaces.removeValue(forKey: id)?.end() }

  func endAll() {
    for surface in surfaces.values { surface.end() }
    surfaces = [:]
  }

  /// A web link the shell printed opens in the browser; any other scheme (`file:`, an app's
  /// own) stays shut, since the text came from whatever ran in the terminal.
  private static func open(_ link: String) {
    guard let url = URL(string: link), ["http", "https"].contains(url.scheme?.lowercased())
    else { return }
    NSWorkspace.shared.open(url)
  }
}

/// The pane: the tabs' header over the shown tab's terminal.
struct SessionTerminal: View {
  /// Mockup 24's `.term` height, until the user drags the header.
  static let defaultHeight: CGFloat = 250
  /// The least the well shrinks to: a few lines.
  static let minHeight: CGFloat = 120

  let store: SessionStore
  let surfaces: TerminalSurfaces
  /// The session's linked worktree's branch, for the tab titles.
  let branch: String?
  @Binding var height: CGFloat
  let refused: (String) -> Void
  @Environment(\.coxAppearance) private var appearance
  @State private var dragStart: CGFloat?

  var body: some View {
    TerminalPaneChrome(state: state, send: handle) {
      // `height` is the well's, margin included, as mockup 24's `.term` is 250 under its header.
      ZStack {
        if let tab = store.terminals.first(where: { $0.id == store.terminalSelection }) {
          TerminalPane(surface: surfaces.surface(for: tab, style: style), style: style)
            .id(tab.id)
        }
      }
      .frame(maxWidth: .infinity)
      .frame(height: height - 2 * Space.ml)
    }
    // The chrome's well is greedy; at its ideal height it keeps to the header and `height`.
    .fixedSize(horizontal: false, vertical: true)
    .overlay(alignment: .top) { resizeEdge }
  }

  private var state: TerminalPaneState {
    // The login shell's name; the core takes the same `$SHELL`, or its default when it is off
    // the allowlist.
    let shell = ProcessInfo.processInfo.environment["SHELL"] ?? "zsh"
    return TerminalPaneState(
      tabs: store.terminals.map {
        TerminalPaneTab(id: $0.id, title: TerminalTab.title(shell: shell, branch: branch))
      },
      selection: store.terminalSelection)
  }

  /// The terminal's own background is `surface.terminal` at no opacity: the chrome's well already
  /// lays that colour once, margin included, and a second coat inside the margin
  /// drew a lighter frame round a darker terminal. The colour stays for inverse video.
  /// Lines keep the token's pitch (`font.mono.terminal`, 1.5), not the font's tighter own.
  private var style: TerminalStyle {
    let token = FontToken.monoTerminal
    return TerminalStyle(
      font: token.nsFont(scale: appearance.textScale),
      foreground: TerminalColour.text.nsColor,
      background: TerminalColour.surface.nsColor.withAlphaComponent(0),
      caret: TerminalColour.text.nsColor,
      lineHeight: token.size * appearance.textScale * token.lineHeight)
  }

  /// The pane's top edge drags its height; UI-only state, never written to config.
  private var resizeEdge: some View {
    Color.clear
      .frame(height: Space.xs)
      .contentShape(Rectangle())
      .pointerStyle(.frameResize(position: .top))
      .gesture(
        DragGesture(coordinateSpace: .global)
          .onChanged { drag in
            let start = dragStart ?? height
            dragStart = start
            height = max(Self.minHeight, start - drag.translation.height)
          }
          .onEnded { _ in dragStart = nil })
  }

  private func handle(_ intent: TerminalPaneIntent) {
    switch intent {
    case .select(let id): store.terminalSelection = id
    case .add:
      do {
        try store.openTerminal()
      } catch {
        refused(String(describing: error))
      }
    case .close(let id):
      surfaces.end(id)
      store.closeTerminal(id)
    }
  }
}

extension View {
  /// Asks before the window closes while `isBusy` says a terminal runs a foreground job.
  func closeGuard(isBusy: @escaping @MainActor () -> Bool) -> some View {
    background(TerminalCloseGuard(isBusy: isBusy))
  }
}

/// Puts `CloseGuard` between the window and SwiftUI's own delegate once the view is in it.
private struct TerminalCloseGuard: NSViewRepresentable {
  let isBusy: @MainActor () -> Bool

  func makeNSView(context: Context) -> Probe { Probe(isBusy: isBusy) }
  func updateNSView(_ probe: Probe, context: Context) { probe.guardian.isBusy = isBusy }

  final class Probe: NSView {
    let guardian: CloseGuard

    init(isBusy: @escaping @MainActor () -> Bool) {
      guardian = CloseGuard(isBusy: isBusy)
      super.init(frame: .zero)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    override func viewDidMoveToWindow() {
      super.viewDidMoveToWindow()
      guard let window, window.delegate !== guardian else { return }
      guardian.original = window.delegate
      window.delegate = guardian
    }
  }
}

/// The window's delegate while the probe lives: asks in a sheet before closing over a busy
/// terminal, and hands every other delegate message to the delegate SwiftUI set.
private final class CloseGuard: NSObject, NSWindowDelegate {
  /// Read by the runtime's `responds(to:)` and forwarding, which are not main-actor methods;
  /// set once, on the main actor, before the window sees this delegate.
  nonisolated(unsafe) weak var original: (any NSWindowDelegate)?
  var isBusy: @MainActor () -> Bool

  init(isBusy: @escaping @MainActor () -> Bool) {
    self.isBusy = isBusy
  }

  override func responds(to selector: Selector!) -> Bool {
    super.responds(to: selector) || original?.responds(to: selector) == true
  }

  override func forwardingTarget(for selector: Selector!) -> Any? {
    original?.responds(to: selector) == true ? original : nil
  }

  func windowShouldClose(_ window: NSWindow) -> Bool {
    guard isBusy() else { return original?.windowShouldClose?(window) ?? true }
    let alert = NSAlert()
    alert.messageText = "Close this window?"
    alert.informativeText =
      "A command is still running in a terminal. Closing the window ends it."
    alert.addButton(withTitle: "Close")
    alert.addButton(withTitle: "Cancel")
    alert.beginSheetModal(for: window) { response in
      // `close()` skips this check, which already asked.
      if response == .alertFirstButtonReturn { window.close() }
    }
    return false
  }
}
