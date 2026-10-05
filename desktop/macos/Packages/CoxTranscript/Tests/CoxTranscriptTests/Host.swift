// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The harness every suite here shares: a `TranscriptView` over a store in a borderless window
// far off screen, ordered in so AppKit lays it out and draws it, with real `NSEvent` drags and
// copies into private pasteboards — as spike T37.37 and T37.42 drove the text view, but through
// the SwiftUI view the app hosts. Separate so the selection, snapshot and benchmark suites
// drive one host.

import AppKit
import CoxClient
import CoxModel
import CoxTranscript
import CoxTranscriptText
import CoxUI
import QuartzCore
import SwiftUI

@MainActor
final class Host {
  let store: SessionStore
  let window: NSWindow
  let hosting: NSHostingView<AnyView>
  private let material: GlassMaterial
  /// Set, the approval slot holds the real `DecisionCard`s and their intents go here.
  private let send: (@MainActor (Intent) -> Void)?
  /// Where a prompt's Edit and resend goes; `nil` offers Copy only.
  var composer: ComposerStore?

  init(
    _ blocks: [Block], size: NSSize, crossBlockSelection: Bool = true, dark: Bool = false,
    material: GlassMaterial = .solid, send: (@MainActor (Intent) -> Void)? = nil
  ) {
    NSApplication.shared.setActivationPolicy(.accessory)
    store = SessionStore(session: FixtureSession(fixture: Fixture(batches: [], snapshot: [])))
    store.apply([.reset(blocks: blocks)])
    self.material = material
    self.send = send
    hosting = NSHostingView(rootView: AnyView(EmptyView()))
    window = NSWindow(
      // A window frame far off screen, not a design size.
      // swiftlint:disable:next no_literal_size
      contentRect: NSRect(origin: NSPoint(x: -20_000, y: -20_000), size: size),
      styleMask: [.borderless], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
    window.backgroundColor = dark ? .black : .white
    window.contentView = hosting
    show(crossBlockSelection: crossBlockSelection)
    window.orderFrontRegardless()
    flush()
  }

  /// Hands the view a new setting, as the app does when the user flips it, the text size or
  /// `[desktop.transcript]`'s `text_size` and `line_height` (`text`).
  func show(
    crossBlockSelection: Bool, textScale: Double = 1,
    text: (size: Double, lineHeight: Double)? = nil
  ) {
    let text = text ?? (Double(FontToken.transcript.size), Double(FontToken.transcript.lineHeight))
    let transcript =
      if let send {
        AnyView(
          TranscriptView(store: store, crossBlockSelection: crossBlockSelection, send: send)
            .composer(composer)
            .text(size: text.size, lineHeight: text.lineHeight))
      } else {
        AnyView(
          TranscriptView(store: store, crossBlockSelection: crossBlockSelection) { block in
            Text(verbatim: Self.slot(block))
          }
          .composer(composer)
          .text(size: text.size, lineHeight: text.lineHeight))
      }
    hosting.rootView = AnyView(
      transcript
        .environment(\.coxAppearance, Appearance(material: material, textScale: textScale))
        // Durations read the same on every machine.
        .environment(\.locale, Locale(identifier: "en_US_POSIX")))
    flush()
  }

  /// What the approval slot shows when the host is given no `send`.
  static func slot(_ block: Block) -> String {
    switch block.kind {
    case .approval(_, _, let summary, _, _, _, _, _, _): "Approve: \(summary)"
    case .question(_, let question, _, _): "Asked: \(question)"
    default: ""
    }
  }

  /// The one transcript the host holds.
  var text: TranscriptTextView {
    descendants(of: hosting, as: TranscriptTextView.self).first!
  }

  var scroll: NSScrollView? { text.enclosingScrollView }

  /// One frame: layout, draw and commit. An occluded window draws no layers, and TextKit 2
  /// makes a card's view when its line draws, so the text view draws `dirty` (all it shows
  /// when `nil`) into a bitmap itself, as the T37.41 suite does.
  func flush(drawing dirty: NSRect? = nil) {
    window.layoutIfNeeded()
    window.displayIfNeeded()
    for text in descendants(of: hosting, as: TranscriptTextView.self) {
      let rect = dirty ?? text.visibleRect
      let bitmap = text.bitmapImageRepForCachingDisplay(in: rect)
      bitmap.map { text.cacheDisplay(in: rect, to: $0) }
    }
    CATransaction.flush()
  }

  /// What a frame redraws while a reply streams at the bottom: the shown text from the top of
  /// its last paragraph down.
  var streamedTail: NSRect {
    let visible = text.visibleRect
    guard let manager = text.textLayoutManager,
      let last = manager.location(manager.documentRange.endLocation, offsetBy: -1),
      let fragment = manager.textLayoutFragment(for: last)
    else { return visible }
    let top = max(visible.minY, fragment.layoutFragmentFrame.minY + text.textContainerOrigin.y)
    return NSRect(x: visible.minX, y: top, width: visible.width, height: visible.maxY - top)
  }

  /// Turns the run loop until every card on screen has its view placed (TextKit places a
  /// card's view one turn after it draws the line, T37.41), or `limit` passes.
  func settle(limit: TimeInterval = 5) {
    let deadline = Date(timeIntervalSinceNow: limit)
    var last = -1
    repeat {
      flush()
      RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.01))
      // A card's view is placed as a subview of the text view's fragments.
      let placed = descendants(of: text, as: NSView.self).count
      if placed == last { break }
      last = placed
    } while Date() < deadline
    flush()
  }

  /// The window point in the middle of the character `offset` into `block`.
  func point(_ block: BlockID, _ offset: Int) -> NSPoint {
    let start = text.range(of: block)?.location ?? 0
    let screen = text.firstRect(
      forCharacterRange: NSRange(location: start + offset, length: 1), actualRange: nil)
    let rect = window.convertFromScreen(screen)
    return NSPoint(x: rect.midX, y: rect.midY)
  }

  /// One drag: mouse down, eight drags, mouse up, each sent through the window as AppKit
  /// sends a hand drag's events.
  func drag(from start: NSPoint, to end: NSPoint) {
    let steps = 8
    for step in 0...steps + 1 {
      let type: NSEvent.EventType =
        step == 0 ? .leftMouseDown : step > steps ? .leftMouseUp : .leftMouseDragged
      let share = CGFloat(min(step, steps)) / CGFloat(steps)
      let point = NSPoint(
        x: start.x + (end.x - start.x) * share, y: start.y + (end.y - start.y) * share)
      let event = NSEvent.mouseEvent(
        with: type, location: point, modifierFlags: [],
        timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
        context: nil, eventNumber: step, clickCount: 1, pressure: type == .leftMouseUp ? 0 : 1)
      if let event { window.sendEvent(event) }
    }
  }

  /// The blocks the selection touches, in order.
  var selectedBlocks: [BlockID] {
    let selected = text.selectedRange()
    return text.blockRanges.ids.filter {
      guard let range = text.range(of: $0) else { return false }
      return NSIntersectionRange(range, selected).length > 0
    }
  }

  /// Copies the selection into a private pasteboard and reads both types back.
  func copy() -> (markdown: String?, plain: String?) {
    let board = NSPasteboard(name: NSPasteboard.Name("cox.transcript.tests.\(UUID())"))
    defer { board.releaseGlobally() }
    _ = text.writeSelection(to: board, types: text.writablePasteboardTypes)
    return (board.string(forType: .markdown), board.string(forType: .string))
  }

  /// Sizes the window to the whole text, so an image holds every block.
  func fitToText() {
    guard let manager = text.textLayoutManager else { return }
    manager.ensureLayout(for: manager.documentRange)
    let height = manager.usageBoundsForTextContainer.height + 2 * text.textContainerInset.height
    window.setContentSize(NSSize(width: window.frame.width, height: ceil(height)))
    settle()
  }

  /// The window's content over its background, as an image of a fixed 2× scale.
  func image() throws -> NSImage {
    let view = hosting
    guard let content = bitmap(view.bounds.size), let flat = bitmap(view.bounds.size),
      let context = NSGraphicsContext(bitmapImageRep: flat)?.cgContext
    else { throw HostError.noBitmap }
    view.cacheDisplay(in: view.bounds, to: content)
    guard let drawn = content.cgImage else { throw HostError.noBitmap }
    // The view draws no background of its own (it sits on the app's pane); the window's colour
    // stands in, so the image shows the text as a reader does.
    // The context maps points to the 2× pixels; drawing through it composites over the fill,
    // where `NSImageRep.draw` would copy the transparent pixels over it.
    context.setFillColor(window.backgroundColor.cgColor)
    context.fill(view.bounds)
    context.draw(drawn, in: view.bounds)
    context.flush()
    let image = NSImage(size: flat.size)
    image.addRepresentation(flat)
    return image
  }

  private func bitmap(_ size: NSSize) -> NSBitmapImageRep? {
    let bitmap = NSBitmapImageRep(
      bitmapDataPlanes: nil, pixelsWide: Int(size.width) * 2, pixelsHigh: Int(size.height) * 2,
      bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
      colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)
    bitmap?.size = size
    return bitmap
  }

  func close() {
    window.orderOut(nil)
    window.close()
  }

  enum HostError: Error { case noBitmap }
}

func descendants<T: NSView>(of view: NSView, as type: T.Type) -> [T] {
  var found: [T] = []
  var stack: [NSView] = [view]
  while let next = stack.popLast() {
    if let match = next as? T { found.append(match) }
    stack.append(contentsOf: next.subviews)
  }
  return found
}

/// Whether views can be driven by synthesized mouse events. Before macOS 27, NSTextView and
/// SwiftUI's controls track a press in a modal loop that pulls the drag and the release from the
/// application's queue: sent after the press they never arrive and the test hangs; queued ahead
/// of it, they end the test runner's main run loop and the process exits 0 mid-run.
let syntheticMouse = ProcessInfo.processInfo.isOperatingSystemAtLeast(
  OperatingSystemVersion(majorVersion: 27, minorVersion: 0, patchVersion: 0))
