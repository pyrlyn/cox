// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A new text size restyles the text in place (T37.23.6): every character takes the look a
// fresh load at that size gives it — fonts, a prompt's and a thought's decor, a reply's
// structure — while the characters, the block ranges, the selection, an open thought and
// each card's attachment stay as they were.

import AppKit
import CoxClient
import Testing

@testable import CoxTranscriptText

private func span(_ text: String, bold: Bool = false) -> Span {
  var span = Span(text: text)
  span.bold = bold
  return span
}

private let blocks: [Block] = [
  Block(id: "u", turn: 1, kind: .user(text: "Fix the watcher test.", attachments: ["log.txt"])),
  Block(id: "k", turn: 1, kind: .thinking(text: "It sleeps.\nWait for the event.")),
  tool("t", "Ran cargo nextest run"),
  Block(
    id: "a", turn: 1,
    kind: .assistant(
      text: "",
      doc: StyledDoc(blocks: [
        .text(kind: .heading(2), lines: [TextLine([span("Fix", bold: true)])]),
        .text(kind: .list, lines: [TextLine([span("wait for the event")], marker: "•")]),
        .text(kind: .quote, lines: [TextLine([span("no more sleeps")], quote: 1)]),
        .code(lang: "rust", lines: [[span("rx.recv().await?;")]]),
        .rule,
        .table(rows: [["test", "state"], ["watcher", "ok"]]),
      ]))),
]

/// The system style at `scale` times its size, with every size-bound value following it.
@MainActor
private func style(_ scale: CGFloat) -> TranscriptStyle {
  let base = TranscriptStyle.system
  func sized(_ font: NSFont) -> NSFont {
    NSFont(descriptor: font.fontDescriptor, size: font.pointSize * scale) ?? font
  }
  let size = base.body.pointSize * scale
  return TranscriptStyle(
    body: sized(base.body), code: sized(base.code), text: base.text, blockSpacing: size / 2,
    inset: NSSize(width: size, height: size),
    bubble: .init(
      fill: base.bubble.fill, radius: size / 2, padding: NSSize(width: size, height: size / 2),
      gap: size / 3),
    thought: .init(
      font: sized(base.thought.font), color: base.thought.color, rule: base.thought.rule,
      ruleWidth: base.thought.ruleWidth, indent: size),
    indent: size * 1.5)
}

/// Each character's look, attachments aside: those a restyle keeps.
@MainActor
private func looks(_ view: TranscriptTextView) -> [String] {
  guard let storage = view.textStorage else { return [] }
  return (0..<storage.length).map { index in
    let found = storage.attributes(at: index, effectiveRange: nil)
    let font = (found[.font] as? NSFont).map { "\($0.fontName) \($0.pointSize)" }
    let decor = (found[.transcriptDecor] as? Decor).map { "\($0.kind)" }
    let own = (found[.transcriptParagraph] as? Paragraph)?.own
    let values: [Any?] = [
      font, found[.foregroundColor], found[.paragraphStyle], decor, found[.transcriptEdge], own,
      found[.kern], found[.attachment].map { type(of: $0) },
    ]
    return values.map { $0.map { "\($0)" } ?? "-" }.joined(separator: " | ")
  }
}

@MainActor
private func attachment(
  _ view: TranscriptTextView, _ id: BlockID, _ offset: Int = 0
) -> AnyObject? {
  guard let range = view.range(of: id) else { return nil }
  return view.textStorage?.attribute(.attachment, at: range.location + offset, effectiveRange: nil)
    as AnyObject?
}

@MainActor
@Suite(.serialized)
struct TranscriptRestyleTests {
  let larger = style(1.5)

  @Test func aNewStyleRestylesEveryCharacterAndKeepsTheTextTheSelectionAndTheCards() throws {
    let view = TranscriptTextView.make(style: style(1))
    view.load(blocks)
    view.setThought("k", open: true)
    let text = view.string
    let ranges = view.blockRanges
    let prompt = try #require(view.range(of: "u"))
    let reply = try #require(view.range(of: "a"))
    let selection = NSRange(location: prompt.location + 4, length: reply.location + 6)
    view.setSelectedRange(selection)
    let tile = try #require(attachment(view, "u", prompt.length - 1))
    let (header, card) = try (#require(attachment(view, "k")), #require(attachment(view, "t")))

    view.restyle(larger)

    #expect(view.string == text)
    #expect(view.blockRanges == ranges)
    #expect(view.selectedRange() == selection)
    #expect(view.openThoughts == ["k"])
    #expect(view.textContainerInset == larger.inset)
    #expect(attachment(view, "u", prompt.length - 1) === tile, "a prompt's tile keeps its view")
    #expect(attachment(view, "k") === header, "a thought's header keeps its view")
    #expect(attachment(view, "t") === card, "a card keeps its view")
    let font = view.textStorage?.attribute(.font, at: reply.location + 3, effectiveRange: nil)
    #expect((font as? NSFont)?.pointSize == larger.body.pointSize, "a heading takes the new size")

    let fresh = TranscriptTextView.make(style: larger)
    fresh.openThoughts = ["k"]
    fresh.load(blocks)
    #expect(looks(view) == looks(fresh))
    #expect(view.docStarts == fresh.docStarts)
  }

  @Test func theSameStyleLeavesTheTextAlone() throws {
    let view = TranscriptTextView.make(style: style(1))
    view.load(blocks)
    let storage = try #require(view.textStorage)
    let edits = Edits(storage)

    view.restyle(style(1))

    #expect(edits.ranges.isEmpty)
  }
}
