// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Cards as view-backed attachments (T37.41's Check): a card that expands and
// collapses moves the block below it without touching its range, and a drag
// across a card selects the card whole. Run in an off-screen window, with real
// mouse events sent through it, as spike T37.37 measured (research.md §9.5.13).

import AppKit
import CoxClient
import SwiftUI
import Testing

@testable import CoxTranscriptText

@Observable @MainActor
final class Fold {
  var open = false
}

/// A card as tall as its fold says: 40 pt closed, 200 pt open.
struct FoldCard: View {
  let fold: Fold
  var body: some View { Color.clear.frame(height: fold.open ? 200 : 40) }
}

func tool(_ id: BlockID, _ summary: String) -> Block {
  Block(
    id: id, turn: 1,
    kind: .tool(
      tool: "bash", summary: summary, icon: .shell, risk: .exec, state: .done, tail: "",
      archive: nil, diff: nil, durationMs: 1))
}

/// A borderless window far off screen, ordered in so AppKit lays out and draws it.
@MainActor
final class Offscreen {
  let window: NSWindow
  let view: TranscriptTextView

  init(_ view: TranscriptTextView, blocks: [Block]) {
    NSApplication.shared.setActivationPolicy(.accessory)
    window = NSWindow(
      // swiftlint:disable:next no_literal_size
      contentRect: NSRect(x: -20_000, y: -20_000, width: 600, height: 500),
      styleMask: [.borderless], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    self.view = view
    window.contentView = view.inScrollView(frame: window.contentLayoutRect)
    view.load(blocks)
    window.orderFrontRegardless()
    flush()
  }

  /// One frame. An occluded window draws no layers, and TextKit 2 adds a
  /// card's view when its line draws, so the text view draws into a bitmap.
  func flush() {
    window.layoutIfNeeded()
    window.displayIfNeeded()
    if let bitmap = view.bitmapImageRepForCachingDisplay(in: view.visibleRect) {
      view.cacheDisplay(in: view.visibleRect, to: bitmap)
    }
    CATransaction.flush()
  }

  /// Spins the run loop until `done` holds or two seconds pass.
  func wait(until done: () -> Bool) -> Bool {
    let deadline = Date(timeIntervalSinceNow: 2)
    while Date() < deadline {
      flush()
      if done() { return true }
      RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.005))
    }
    return done()
  }

  /// Where `block`'s layout fragment sits in the text view.
  func frame(of block: BlockID) -> CGRect? {
    guard let range = view.range(of: block), let start = view.textRange(range)?.location
    else { return nil }
    return view.textLayoutManager?.textLayoutFragment(for: start)?.layoutFragmentFrame
  }

  /// A window point on the first line of `block`, `offset` characters in.
  func point(in block: BlockID, offset: Int) -> NSPoint {
    let location = (view.range(of: block)?.location ?? 0) + offset
    let screen = view.firstRect(
      forCharacterRange: NSRange(location: location, length: 1), actualRange: nil)
    let rect = window.convertFromScreen(screen)
    return NSPoint(x: rect.midX, y: rect.midY)
  }

  /// One drag of real mouse events, as AppKit delivers a hand drag.
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
      event.map(window.sendEvent)
    }
  }

  /// The one card's view once it is in the window and the launch passes are done with it.
  /// A pass that ran before TextKit drew the card's line marked it for a re-layout
  /// (`placing`) that runs only on a later run-loop turn (`afterDisplay`); the view can be in
  /// the window first, so waiting for the view alone left that follow-up pending into the test.
  func placedCard() throws -> CardHost {
    #expect(
      wait {
        let hosts = cardHosts(in: view)
        return hosts.count == 1 && hosts.allSatisfy { $0.window != nil && !$0.placing }
      }, "the card's view is in place with no re-layout pending")
    return try #require(cardHosts(in: view).first)
  }

  func close() {
    window.orderOut(nil)
    window.close()
  }
}

@MainActor
@Suite(.serialized)
struct TranscriptCardsTests {
  let blocks = [
    Block(id: "a", turn: 1, kind: .notice(level: .info, text: "Before the card, one line.")),
    tool("c", "cargo test"),
    Block(id: "b", turn: 1, kind: .notice(level: .info, text: "After the card, one line.")),
  ]

  @Test func cardIsOneAttachmentCharacterHostingItsView() throws {
    let screen = Offscreen(TranscriptTextView.make(), blocks: blocks)
    defer { screen.close() }
    let range = try #require(screen.view.range(of: "c"))
    let storage = try #require(screen.view.textStorage)

    #expect(range.length == 1)
    let attachment = storage.attribute(.attachment, at: range.location, effectiveRange: nil)
    #expect(attachment is CardAttachment)
    #expect(screen.wait { !cardHosts(in: screen.view).isEmpty }, "the card's view is in the text")
  }

  @Test func expandingAndCollapsingACardMovesTheNextBlockOnly() throws {
    let fold = Fold()
    let view = TranscriptTextView.make()
    view.cards = TranscriptCards { _ in FoldCard(fold: fold) }
    let screen = Offscreen(view, blocks: blocks)
    defer { screen.close() }
    let text = view.string
    let next = try #require(view.range(of: "b"))
    let closed = try #require(screen.frame(of: "b"))
    let card = try #require(screen.frame(of: "c"))

    fold.open = true
    #expect(screen.wait { abs((screen.frame(of: "b")?.minY ?? 0) - closed.minY - 160) < 1 })
    let opened = try #require(screen.frame(of: "b"))
    #expect(abs(opened.minY - closed.minY - 160) < 1, "the next block moves by the card's growth")
    #expect(abs((screen.frame(of: "c")?.height ?? 0) - card.height - 160) < 1)
    #expect(view.range(of: "b") == next && view.string == text, "no text changed")

    fold.open = false
    #expect(screen.wait { screen.frame(of: "b")?.minY == closed.minY })
    #expect(screen.frame(of: "b") == closed)
    #expect(view.range(of: "b") == next)
  }

  /// T37.22.10: a viewport pass that swapped in a new element view for a card's line left the
  /// card's view out, a blank gap on launch until the window was resized. The offscreen window
  /// draws every line anew and puts the view back itself, so this checks the pass's follow-up:
  /// one re-layout of the card's line per turn, none for a card in place.
  @Test func aCardAViewportPassLeftOutIsLaidOutAgainOncePerTurn() throws {
    let screen = Offscreen(TranscriptTextView.make(), blocks: blocks)
    defer { screen.close() }
    let host = try screen.placedCard()
    screen.view.placeCards()
    #expect(!host.placing, "a card in place is left alone")

    host.removeFromSuperview()
    screen.view.placeCards()
    screen.view.placeCards()

    #expect(host.placing && host.placings == 1, "one re-layout for the turn")
    #expect(screen.wait { host.window != nil && !host.placing }, "the card's view is back")
    #expect(host.placings == 0)
  }

  /// T37.22.14: on macOS 26 `NSTextView` may not implement the pass it declares from 27, so
  /// the override must place cards without calling `super`. The controller reports its passes
  /// to the view itself, which is what makes the override run at all.
  @Test func withoutNSTextViewsOwnPassCardsArePlacedAndSuperIsNotCalled() throws {
    let screen = Offscreen(TranscriptTextView.make(), blocks: blocks)
    defer { screen.close() }
    let host = try screen.placedCard()
    let controller = try #require(screen.view.textLayoutManager?.textViewportLayoutController)
    #expect(controller.delegate === screen.view, "the controller calls the view's override")

    host.removeFromSuperview()
    screen.view.viewportDidLayout(controller, superLaysOut: false)

    #expect(host.placing && host.placings == 1, "the macOS 26 path places the card")
    #expect(screen.wait { host.window != nil && !host.placing }, "the card's view is back")
  }

  @Test(.enabled(if: syntheticMouse, "synthesized mouse events need macOS 27"))
  func dragAcrossACardSelectsTheWholeCard() throws {
    let screen = Offscreen(TranscriptTextView.make(), blocks: blocks)
    defer { screen.close() }
    let card = try #require(screen.view.range(of: "c"))

    screen.drag(from: screen.point(in: "a", offset: 3), to: screen.point(in: "b", offset: 5))
    let selected = screen.view.selectedRange()

    #expect(NSIntersectionRange(selected, card) == card, "the card is selected whole")
    #expect(selected.location == (screen.view.range(of: "a")?.location ?? -1) + 3)
    #expect(NSMaxRange(selected) > NSMaxRange(card))
  }
}

@MainActor
func cardHosts(in view: NSView) -> [CardHost] {
  var found: [CardHost] = []
  var stack = [view]
  while let next = stack.popLast() {
    if let host = next as? CardHost { found.append(host) }
    stack.append(contentsOf: next.subviews)
  }
  return found
}
