// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Copy as Markdown and the one-block clamp (T37.42's Check): real `NSEvent`
// drags through an offscreen window, as spike T37.37 drove them, and copy
// into a private named pasteboard — never the general one.

import AppKit
import CoxClient
import Foundation
import Testing

@testable import CoxTranscriptText

private let userText = "Please fix the **flaky** test in cox-core."
let summary = "Ran cargo nextest run -p cox-core"
let replySource = "Fixed it:\n\n```sh\ncargo nextest run -p cox-core\n```"

/// A user message, a tool card, a reply with a code block, then one more block.
let transcript: [Block] = [
  Block(id: "u", turn: 1, kind: .user(text: userText, attachments: [])),
  Block(
    id: "t", turn: 1,
    kind: .tool(
      tool: "bash", summary: summary, icon: .shell, risk: .exec, state: .done, tail: "",
      archive: nil, diff: nil, durationMs: 1_200)),
  Block(
    id: "a", turn: 1,
    kind: .assistant(
      text: replySource,
      doc: StyledDoc(blocks: [
        .text(kind: .paragraph, lines: [TextLine([Span(text: "Fixed it:")])]),
        .code(lang: "sh", lines: [[Span(text: "cargo nextest run -p cox-core")]]),
      ]))),
  Block(id: "k", turn: 2, kind: .thinking(text: "Next turn.")),
]

/// Stands for the core's writer, which these tests do not link: only a code block's fence, the
/// one rule Copy's cut-through-code case leans on; the rest is the plain stand-in's.
struct FencingWriter: DocWriter {
  func markdown(_ doc: StyledDoc) -> String { PlainDocWriter().markdown(doc) }

  func markdown(_ block: DocBlock) -> String? {
    guard case .code(let lang, _) = block, let body = PlainDocWriter().markdown(block) else {
      return PlainDocWriter().markdown(block)
    }
    return "```\(lang)\n\(body)\n```"
  }
}

/// A borderless window far off screen, ordered in so AppKit lays out and
/// draws it, with the transcript as its content.
@MainActor
final class Host {
  let window: NSWindow
  let view: TranscriptTextView

  init(style: TranscriptStyle = .system) {
    DocMarkdown.writer = FencingWriter()
    view = TranscriptTextView.make(style: style)
    NSApplication.shared.setActivationPolicy(.accessory)
    window = NSWindow(
      // A window frame far off screen, not a design size.
      // swiftlint:disable:next no_literal_size
      contentRect: NSRect(x: -20_000, y: -20_000, width: 700, height: 500),
      styleMask: [.borderless], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = view.inScrollView(frame: NSRect(origin: .zero, size: window.frame.size))
    // The last block is an open thought, so its reasoning is text to select and copy.
    view.openThoughts = ["k"]
    view.load(transcript)
    window.orderFrontRegardless()
    window.layoutIfNeeded()
    window.displayIfNeeded()
  }

  /// The window point in the middle of the character `offset` into `block`.
  func point(_ block: BlockID, _ offset: Int) -> NSPoint {
    let start = view.range(of: block)?.location ?? 0
    let screen = view.firstRect(
      forCharacterRange: NSRange(location: start + offset, length: 1), actualRange: nil)
    let rect = window.convertFromScreen(screen)
    return NSPoint(x: rect.midX, y: rect.midY)
  }

  /// One drag: mouse down, eight drags, mouse up, each sent through the
  /// window as AppKit sends a hand drag's events.
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
    let selected = view.selectedRange()
    return view.blockRanges.ids.filter {
      guard let range = view.range(of: $0) else { return false }
      return NSIntersectionRange(range, selected).length > 0
    }
  }

  /// Copies the selection into a private pasteboard and reads both types back.
  func copy() -> (markdown: String?, plain: String?) {
    let board = NSPasteboard(name: NSPasteboard.Name("cox.transcript-text.tests.\(UUID())"))
    defer { board.releaseGlobally() }
    _ = view.writeSelection(to: board, types: view.writablePasteboardTypes)
    return (board.string(forType: .markdown), board.string(forType: .string))
  }

  func close() {
    window.orderOut(nil)
    window.close()
  }
}

/// The code line's offset inside the reply block.
private let codeOffset = ("Fixed it:\n" as NSString).length + 6

@MainActor
@Suite(.serialized)
struct SelectionTests {
  @Test(.enabled(if: syntheticMouse, "synthesized mouse events need macOS 27"))
  func dragAcrossThreeBlocksCopiesTheirMarkdownInOrder() throws {
    let host = Host()
    defer { host.close() }
    host.drag(from: host.point("u", 7), to: host.point("a", codeOffset))

    #expect(host.selectedBlocks == ["u", "t", "a"])
    let (markdown, plain) = host.copy()
    let copied = try #require(markdown)
    let parts = [
      "fix the **flaky** test in cox-core.\n\n", summary + "\n\n", "Fixed it:\n\n```sh\ncargo",
    ]
    let found = try parts.map { try #require(copied.range(of: $0), "\($0) in \(copied)") }
    #expect(found.map(\.lowerBound) == found.map(\.lowerBound).sorted())
    #expect(copied.hasSuffix("\n```"), "a code block cut short stays fenced")
    #expect(!copied.contains("Next turn"))
    let text = try #require(plain)
    #expect(text.contains("cox-core.\n\(summary)\nFixed it:\ncargo"))
    #expect(!text.contains("```"))
  }

  @Test(.enabled(if: syntheticMouse, "synthesized mouse events need macOS 27"))
  func withTheSettingOffTheDragStaysInItsFirstBlockBothWays() throws {
    let host = Host()
    defer { host.close() }
    host.view.crossBlockSelection = false

    host.drag(from: host.point("u", 7), to: host.point("a", codeOffset))
    #expect(host.selectedBlocks == ["u"])
    let markdown = try #require(host.copy().markdown)
    #expect(!markdown.isEmpty && userText.hasSuffix(markdown))

    host.drag(from: host.point("a", codeOffset), to: host.point("u", 7))
    #expect(host.selectedBlocks == ["a"])
    #expect(host.copy().markdown?.hasPrefix("Fixed it:\n\n```sh\ncargo") == true)
  }

  @Test func wholeBlocksCopyAsTheirSourceAndCardsAsTheirSummary() {
    let view = TranscriptTextView.make()
    view.openThoughts = ["k"]
    view.load(transcript)
    view.setSelectedRange(NSRange(location: 0, length: (view.string as NSString).length))

    let copied = view.copiedSelection()
    #expect(
      copied.markdown == [userText, summary, replySource, "Next turn."].joined(separator: "\n\n"))
    #expect(
      copied.plain
        == [userText, summary, "Fixed it:\ncargo nextest run -p cox-core", "Next turn."]
        .joined(separator: "\n"))
  }

  @Test func settledSelectionStaysInItsBlockWithTheSettingOff() throws {
    let view = TranscriptTextView.make()
    view.load(transcript)
    view.crossBlockSelection = false
    let reply = try #require(view.range(of: "a"))
    view.setSelectedRange(NSRange(location: reply.location + 2, length: 0))

    view.selectAll(nil)
    #expect(view.selectedRange() == reply)
  }
}

/// Whether views can be driven by synthesized mouse events. Before macOS 27, NSTextView and
/// SwiftUI's controls track a press in a modal loop that pulls the drag and the release from the
/// application's queue: sent after the press they never arrive and the test hangs; queued ahead
/// of it, they end the test runner's main run loop and the process exits 0 mid-run.
let syntheticMouse = ProcessInfo.processInfo.isOperatingSystemAtLeast(
  OperatingSystemVersion(majorVersion: 27, minorVersion: 0, patchVersion: 0))
