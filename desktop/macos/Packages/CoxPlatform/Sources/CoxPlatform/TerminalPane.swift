// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The terminal pane's view (T51.5, DT§3.2, mockup 24): SwiftTerm's `TerminalView` — the
// emulator and renderer only, never `LocalProcessTerminalView`, because Swift never spawns a
// process (DT§4.6) — bridged to a `TerminalClient`. Keys and pastes the view encodes go to
// `write`, its new size in cells to `resize`, and one task feeds the shell's bytes in. In
// CoxPlatform because it is the one package that may link a platform library (DT§4.6); the
// colours and font arrive as a `TerminalStyle` the app builds from CoxUI's tokens. A
// `TerminalSurface` keeps each tab's view and feed alive while SwiftUI rebuilds the pane.

import AppKit
import CoxClient
@preconcurrency import SwiftTerm
import SwiftUI

/// How the pane draws: the mono font token and the `surface.terminal`, `text.terminal` and
/// `text.terminalOk` colours, resolved by the caller. `lineHeight` is the token's line pitch in
/// points; `nil` keeps the font's own, which SwiftTerm uses unless told.
public struct TerminalStyle {
  public var font: NSFont
  public var foreground: NSColor
  public var background: NSColor
  public var caret: NSColor
  public var lineHeight: CGFloat?

  public init(
    font: NSFont, foreground: NSColor, background: NSColor, caret: NSColor,
    lineHeight: CGFloat? = nil
  ) {
    (self.font, self.foreground, self.background, self.caret, self.lineHeight) = (
      font, foreground, background, caret, lineHeight
    )
  }

  @MainActor func apply(to view: TerminalView) {
    if view.font != font { view.font = font }
    // SwiftTerm spaces lines as a multiple of the font's ascent, descent and leading.
    let natural = font.ascender - font.descender + font.leading
    let spacing = lineHeight.map { natural > 0 ? $0 / natural : 1 } ?? 1
    if view.lineSpacing != spacing { view.lineSpacing = spacing }
    view.nativeForegroundColor = foreground
    view.nativeBackgroundColor = background
    view.caretColor = caret
  }
}

/// One tab's terminal, kept by the pane's owner for as long as the tab is open (T51.6): the
/// view holds the scrollback and the feed task holds the output stream, which ends for good once
/// its reader is cancelled, so neither may go when SwiftUI drops the pane on a tab switch or a
/// toggle. `openLink` receives a link the user clicked in the output; by default nothing opens,
/// since the host opens web links only after its own check.
@MainActor
public final class TerminalSurface {
  public let client: any TerminalClient
  let view: TerminalView
  let bridge: TerminalBridge

  public init(
    client: any TerminalClient, style: TerminalStyle,
    openLink: @escaping @MainActor (String) -> Void = { _ in }
  ) {
    self.client = client
    // A zero frame starts SwiftTerm at its default 80 × 25 cells until the pane lays it out.
    view = TerminalView(frame: .zero, font: style.font)
    bridge = TerminalBridge(client: client, openLink: openLink)
    style.apply(to: view)
    bridge.attach(view)
  }

  /// Stops feeding the view; the owner closes the shell.
  public func end() { bridge.detach() }
}

/// A tab's `TerminalSurface` in SwiftUI: a plain container the surface's view moves into, so a
/// pane rebuilt by SwiftUI shows the same terminal rather than a fresh one.
public struct TerminalPane: NSViewRepresentable {
  let surface: TerminalSurface
  let style: TerminalStyle

  public init(surface: TerminalSurface, style: TerminalStyle) {
    (self.surface, self.style) = (surface, style)
  }

  public func makeNSView(context: Context) -> NSView {
    let container = NSView()
    surface.view.removeFromSuperview()
    surface.view.frame = container.bounds
    surface.view.autoresizingMask = [.width, .height]
    container.addSubview(surface.view)
    return container
  }

  public func updateNSView(_ container: NSView, context: Context) {
    style.apply(to: surface.view)
  }

  public static func dismantleNSView(_ container: NSView, coordinator: ()) {
    for view in container.subviews { view.removeFromSuperview() }
  }
}

/// The delegate between the view and the client, and the task that feeds the view.
@MainActor
public final class TerminalBridge {
  let client: any TerminalClient
  let openLink: @MainActor (String) -> Void
  private var feed: Task<Void, Never>?

  init(client: any TerminalClient, openLink: @escaping @MainActor (String) -> Void) {
    (self.client, self.openLink) = (client, openLink)
  }

  /// Makes this the view's delegate and starts feeding it the shell's output.
  func attach(_ view: TerminalView) {
    view.terminalDelegate = self
    feed?.cancel()
    feed = Task { [client, weak view] in
      for await bytes in client.outputs {
        guard let view, !Task.isCancelled else { return }
        view.feed(byteArray: bytes[...])
      }
    }
  }

  /// Stops feeding; the pane's owner decides whether the shell closes.
  func detach() {
    feed?.cancel()
    feed = nil
  }
}

extension TerminalBridge: @preconcurrency TerminalViewDelegate {
  public func send(source: TerminalView, data: ArraySlice<UInt8>) {
    // A write fails only once the shell is gone; the view shows its end.
    try? client.write(Array(data))
  }

  public func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) {
    try? client.resize(cols: UInt16(clamping: newCols), rows: UInt16(clamping: newRows))
  }

  public func requestOpenLink(source: TerminalView, link: String, params: [String: String]) {
    openLink(link)
  }

  public func setTerminalTitle(source: TerminalView, title: String) {}

  public func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}

  public func scrolled(source: TerminalView, position: Double) {}

  public func rangeChanged(source: TerminalView, startY: Int, endY: Int) {}

  /// No program in the pane may read the user's clipboard (OSC 52).
  public func clipboardRead(source: TerminalView) -> Data? { nil }
}
