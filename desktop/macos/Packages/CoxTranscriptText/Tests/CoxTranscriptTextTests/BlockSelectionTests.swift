// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// ⇧-click block selection in the gutter and the "Copy as Markdown" menu item
// (T37.42.2's Check): real `NSEvent` clicks through the offscreen window
// `SelectionTests` uses, and copy into a private named pasteboard — never
// the general one.

import AppKit
import CoxClient
import Foundation
import Testing

@testable import CoxTranscriptText

/// The system style with a leading gutter, as the app's 36 pt one (DT§5).
@MainActor
private let gutterStyle: TranscriptStyle = {
  var style = TranscriptStyle.system
  // The design's gutter width, not a token this package holds.
  // swiftlint:disable:next no_literal_size
  style.inset = NSSize(width: 36, height: 0)
  return style
}()

extension Host {
  /// One click in the gutter beside the first line of `block`.
  func clickGutter(_ block: BlockID, shift: Bool = false) {
    let gutter = view.convert(NSPoint(x: view.textContainerOrigin.x / 2, y: 0), to: nil)
    let location = NSPoint(x: gutter.x, y: point(block, 0).y)
    for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
      let event = NSEvent.mouseEvent(
        with: type, location: location, modifierFlags: shift ? [.shift] : [],
        timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
        context: nil, eventNumber: 0, clickCount: 1, pressure: type == .leftMouseUp ? 0 : 1)
      if let event { window.sendEvent(event) }
    }
  }

  /// The range from the start of `first` to the end of `last`.
  func span(_ first: BlockID, _ last: BlockID) -> NSRange {
    let start = view.range(of: first)?.location ?? 0
    return NSRange(location: start, length: NSMaxRange(view.range(of: last) ?? .init()) - start)
  }
}

@MainActor
@Suite(.serialized, .enabled(if: syntheticMouse, "synthesized mouse events need macOS 27"))
struct BlockSelectionTests {
  @Test func shiftClickInTheGutterFromBlockTwoToFourSelectsExactlyThoseThree() {
    let host = Host(style: gutterStyle)
    defer { host.close() }

    host.clickGutter("t")
    #expect(host.selectedBlocks == ["t"])
    #expect(host.view.selectedRange() == host.span("t", "t"))
    host.clickGutter("k", shift: true)
    #expect(host.selectedBlocks == ["t", "a", "k"])
    #expect(host.view.selectedRange() == host.span("t", "k"))

    host.clickGutter("u", shift: true)
    #expect(host.selectedBlocks == ["u", "t"], "the anchor holds; the selection shrinks")
  }

  @Test func shiftClickInTheGutterExtendsFromATextSelection() {
    let host = Host(style: gutterStyle)
    defer { host.close() }
    let reply = host.view.range(of: "a") ?? .init()
    host.view.setSelectedRange(NSRange(location: reply.location + 2, length: 0))

    host.clickGutter("u", shift: true)
    #expect(host.view.selectedRange() == host.span("u", "a"))
  }

  @Test func withTheSettingOffAShiftClickInTheGutterStaysInItsAnchorBlock() {
    let host = Host(style: gutterStyle)
    defer { host.close() }
    host.view.crossBlockSelection = false

    host.clickGutter("t")
    host.clickGutter("k", shift: true)
    #expect(host.selectedBlocks == ["t"])
    #expect(host.view.selectedRange() == host.span("t", "t"))
  }

  @Test func theMenuItemCopiesTheSelectionAsMarkdown() throws {
    let host = Host(style: gutterStyle)
    defer { host.close() }
    let board = NSPasteboard(name: NSPasteboard.Name("cox.transcript-text.tests.\(UUID())"))
    defer { board.releaseGlobally() }
    host.view.markdownPasteboard = board
    let click = try #require(
      NSEvent.mouseEvent(
        with: .rightMouseDown, location: host.point("a", 2), modifierFlags: [],
        timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: host.window.windowNumber,
        context: nil, eventNumber: 0, clickCount: 1, pressure: 1))
    // A right click on text may select the word under it, so the caret is
    // set after the menu is built.
    let first = try #require(host.view.menu(for: click))
    let disabled = try #require(first.items.first { $0.title == "Copy as Markdown" })
    host.view.setSelectedRange(NSRange(location: 0, length: 0))
    #expect(!host.view.validateMenuItem(disabled), "nothing selected, nothing to copy")

    host.clickGutter("t")
    host.clickGutter("k", shift: true)
    let menu = try #require(host.view.menu(for: click))
    let index = try #require(menu.items.firstIndex { $0.title == "Copy as Markdown" })
    #expect(host.view.validateMenuItem(menu.items[index]))
    menu.performActionForItem(at: index)

    let markdown = [summary, replySource, "Next turn."].joined(separator: "\n\n")
    #expect(board.string(forType: .markdown) == markdown)
    #expect(board.string(forType: .string) == markdown)
  }
}
