// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A new text size (T37.23.6's Check): the app hands the view a new `textScale` and the whole
// transcript restyles in place — the fonts grow, a drag's selection and the block ranges stay,
// a card keeps its view, a reader at the bottom stays there — shown at two sizes, through the
// SwiftUI view the app hosts.

import AppKit
import CoxClient
import SnapshotTesting
import Testing

private func line(_ text: String) -> [Span] { [Span(text: text)] }

private func reply(_ id: BlockID, _ blocks: [DocBlock]) -> Block {
  Block(id: id, turn: 1, kind: .assistant(text: "", doc: StyledDoc(blocks: blocks)))
}

private let blocks: [Block] = [
  Block(id: "u", turn: 1, kind: .user(text: "Please fix the flaky watcher test.", attachments: [])),
  Block(id: "k", turn: 1, kind: .thinking(text: "The test sleeps instead of waiting.")),
  Block(
    id: "e", turn: 1,
    kind: .tool(
      tool: "edit", summary: "Edited watcher.rs", icon: .edit, risk: .write, state: .done, tail: "",
      archive: nil, diff: nil, durationMs: 40)),
  reply(
    "a",
    [
      .text(kind: .heading(2), lines: [TextLine(line("The fix"))]),
      .text(kind: .paragraph, lines: [TextLine(line("The test raced the watcher; it now waits."))]),
      .text(
        kind: .list,
        lines: [TextLine(line("• wait for the first event")), TextLine(line("• drop the sleep"))]),
      .code(lang: "rust", lines: [line("let event = rx.recv().await?;")]),
    ]),
]

// The reading column's width and a window's height, not design sizes.
// swiftlint:disable:next no_literal_size
private let size = NSSize(width: 760, height: 400)

private let larger = 1.3

@MainActor
@Suite(.serialized)
struct TranscriptTextSizeTests {
  private func font(_ host: Host, _ id: BlockID) -> CGFloat? {
    guard let start = host.text.range(of: id)?.location else { return nil }
    let font = host.text.textStorage?.attribute(.font, at: start, effectiveRange: nil)
    return (font as? NSFont)?.pointSize
  }

  @Test(.enabled(if: syntheticMouse, "synthesized mouse events need macOS 27"))
  func aNewTextSizeRestylesTheTextAndKeepsTheSelection() throws {
    let host = Host(blocks, size: size)
    defer { host.close() }
    host.settle()
    host.drag(from: host.point("u", 7), to: host.point("a", 12))
    let selected = host.text.selectedRange()
    #expect(host.selectedBlocks == ["u", "k", "e", "a"])
    let ranges = host.text.blockRanges
    let body = try #require(font(host, "a"))
    let card = try #require(host.text.range(of: "e")).location
    let view = host.text.textStorage?.attribute(.attachment, at: card, effectiveRange: nil)

    host.show(crossBlockSelection: true, textScale: larger)
    host.settle()

    #expect(abs(try #require(font(host, "a")) - body * larger) < 0.01)
    #expect(host.text.selectedRange() == selected)
    #expect(host.text.blockRanges == ranges)
    let kept = host.text.textStorage?.attribute(.attachment, at: card, effectiveRange: nil)
    #expect(kept as AnyObject? === view as AnyObject?, "the card keeps its view")
  }

  @Test func aReaderAtTheBottomStaysThere() throws {
    let sentence = "The watcher test sleeps instead of waiting for the first event. "
    let earlier = (0..<12).map {
      reply("b\($0)", [.text(kind: .paragraph, lines: [TextLine(line(sentence))])])
    }
    let host = Host(earlier + blocks, size: size)
    defer { host.close() }
    host.settle()
    host.scrollToBottom()
    #expect(host.atBottom)

    host.show(crossBlockSelection: true, textScale: larger)
    host.settle()

    #expect(host.atBottom, "offset \(host.offset) of \(host.text.bounds.height)")
  }

  @Test(arguments: [1, larger])
  func atTwoTextSizes(textScale: Double) throws {
    let host = Host(blocks, size: size)
    defer { host.close() }
    host.settle()
    host.show(crossBlockSelection: true, textScale: textScale)
    host.fitToText()
    // Anti-aliasing differs slightly between machines; a real change moves far more pixels.
    assertSnapshot(
      of: try host.image(), as: .image(precision: 0.995, perceptualPrecision: 0.98),
      named: "\(Int(textScale * 100))", testName: "atTwoTextSizes")
  }
}
