// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The session and Settings windows' chrome (DS§3.5, DS§4): no title bar, a see-through window,
// and the behind-window blur under the screen's window pane that draws `[desktop.appearance]`'s
// blur and wallpaper tint. AppKit, so it lives in the app: CoxUI draws the panes, and only the window
// can see the desktop behind it.

import AppKit
import CoxUI
import SwiftUI

/// The desktop behind the window, blurred by `NSVisualEffectView`. Its strength is `blur`, 0…1,
/// shown as the view's opacity over the plain wallpaper, since AppKit offers no blur radius; with
/// `tint` off the view drops its colour, so the wallpaper's hue does not tint the window.
struct BehindWindowBlur: NSViewRepresentable {
  var blur: Double
  var tint: Bool

  func makeNSView(context: Context) -> NSVisualEffectView {
    let view = NSVisualEffectView()
    view.blendingMode = .behindWindow
    // The lightest see-through material: the wallpaper's colour carries through its frost, as
    // the mockup's `.window` blur does, where `.underWindowBackground` greys it out.
    view.material = .fullScreenUI
    view.state = .followsWindowActiveState
    return view
  }

  func updateNSView(_ view: NSVisualEffectView, context: Context) {
    view.alphaValue = blur
  }
}

extension View {
  /// Spreads this view over the whole window, the title bar strip included, on the desktop
  /// behind the window blurred in `shape`: no strip of the window shows the desktop sharp.
  func behindWindowBlur(_ blur: Double, tint: Bool, in shape: some Shape) -> some View {
    ignoresSafeArea().background {
      BehindWindowBlur(blur: blur, tint: tint)
        .saturation(tint ? 1 : 0)
        .clipShape(shape)
        .ignoresSafeArea()
    }
  }

  /// Makes the hosting window see-through, so the panes' glass shows the desktop (DS§4), with
  /// the window buttons on the top row of the pane that starts `paneTop` down: the sidebar's, a
  /// pane gap down, or first run's window pane, which starts at the window's top edge.
  func seeThroughWindow(paneTop: CGFloat = Size.paneGap) -> some View {
    background(SeeThroughWindow(paneTop: paneTop))
  }

  /// Sizes the hosting window's content to `size`, centred, while this view shows, and gives
  /// the window back the frame it had once the view goes: first run sits in the small window
  /// (DS§4 `size.windowSmallWidth`) and the session after it in the window it replaced.
  func windowSize(_ size: NSSize) -> some View { background(WindowSize(size: size)) }
}

private struct WindowSize: NSViewRepresentable {
  let size: NSSize

  func makeNSView(context: Context) -> Probe { Probe(size: size) }
  func updateNSView(_ view: Probe, context: Context) {}

  final class Probe: NSView {
    private let size: NSSize
    /// The frame the window had before this view resized it.
    private var found: NSRect?
    private var leaving: [any NSObjectProtocol] = []

    init(size: NSSize) {
      self.size = size
      super.init(frame: .zero)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    override func viewWillMove(toWindow newWindow: NSWindow?) {
      super.viewWillMove(toWindow: newWindow)
      if newWindow == nil { giveBack() }
    }

    override func viewDidMoveToWindow() {
      super.viewDidMoveToWindow()
      guard let window, found == nil else { return }
      found = window.frame
      window.setContentSize(size)
      window.center()
      // The window's saved frame follows it, so a close or quit mid first run gives the frame
      // back too; otherwise the next launch's session would open at the small size.
      leaving = [
        giveBack(on: NSWindow.willCloseNotification, from: window),
        giveBack(on: NSApplication.willTerminateNotification, from: nil),
      ]
    }

    private func giveBack(on name: Notification.Name, from object: Any?) -> any NSObjectProtocol {
      let centre = NotificationCenter.default
      return centre.addObserver(forName: name, object: object, queue: .main) { [weak self] _ in
        MainActor.assumeIsolated { self?.giveBack() }
      }
    }

    /// Puts the window back at the frame it had, once.
    private func giveBack() {
      for observer in leaving { NotificationCenter.default.removeObserver(observer) }
      leaving = []
      guard let window, let found else { return }
      self.found = nil
      window.setFrame(found, display: true)
    }
  }
}

/// Clears the window's own background once the view is in it and lays the content under the
/// title bar, which the session scene's `.hiddenTitleBar` style does for its window alone; the
/// Settings scene takes no window style, so its window is set up here too. An empty unified
/// toolbar puts the window buttons inside the sidebar pane, off the window's edge, as the
/// mockup's `.traffic` row sits; the system centres them in its 52 pt bar, so they are moved
/// onto the centre of the top row of the pane that holds them, and moved again after each
/// resize, when AppKit lays them out anew.
private struct SeeThroughWindow: NSViewRepresentable {
  let paneTop: CGFloat

  func makeNSView(context: Context) -> Probe { Probe() }
  func updateNSView(_ view: Probe, context: Context) { view.paneTop = paneTop }

  final class Probe: NSView {
    private var resized: (any NSObjectProtocol)?
    var paneTop = Size.paneGap {
      didSet { if paneTop != oldValue { placeButtons() } }
    }

    /// The top row's centre, from the window's top edge: the row is `toolbarHeight - paneGap`
    /// tall, as the sidebar's is.
    private var buttonsCentre: CGFloat { paneTop + (Size.toolbarHeight - Size.paneGap) / 2 }

    override func viewDidMoveToWindow() {
      super.viewDidMoveToWindow()
      if let resized { NotificationCenter.default.removeObserver(resized) }
      guard let window else { return }
      resized = NotificationCenter.default.addObserver(
        forName: NSWindow.didResizeNotification, object: window, queue: .main
      ) { [weak self] _ in MainActor.assumeIsolated { self?.placeButtons() } }
      window.styleMask.insert(.fullSizeContentView)
      window.isOpaque = false
      window.backgroundColor = .clear
      window.titlebarAppearsTransparent = true
      window.titleVisibility = .hidden
      window.titlebarSeparatorStyle = .none
      if window.toolbar == nil { window.toolbar = NSToolbar(identifier: "CoxWindowButtons") }
      window.toolbarStyle = .unified
      placeButtons()
    }

    private func placeButtons() {
      guard let window else { return }
      let centre = window.frame.height - buttonsCentre
      for kind in [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton] {
        guard let button = window.standardWindowButton(kind), let bar = button.superview else {
          continue
        }
        let y = bar.convert(NSPoint(x: 0, y: centre), from: nil).y - button.frame.height / 2
        button.setFrameOrigin(NSPoint(x: button.frame.minX, y: y))
      }
    }
  }
}
