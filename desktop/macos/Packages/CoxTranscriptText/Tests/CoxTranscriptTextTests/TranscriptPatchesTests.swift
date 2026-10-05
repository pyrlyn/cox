// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Incremental text from patches (T37.43's Check): patches give the same text
// a whole load gives, a streamed `AppendText` or `DocTail` edits only its own
// block's range, and 10 000 blocks built patch by patch fit the DT§1 launch
// budget (timed here on a dev machine, not the M1 Air the budget names).

import AppKit
import CoxClient
import Foundation
import QuartzCore
import Testing

@testable import CoxTranscriptText

private func spans(_ text: String, token: StyleToken = .text, bold: Bool = false) -> Span {
  var span = Span(text: text)
  (span.token, span.bold) = (token, bold)
  return span
}

private func reply(_ id: BlockID, _ blocks: [DocBlock]) -> Block {
  Block(id: id, turn: 1, kind: .assistant(text: "", doc: StyledDoc(blocks: blocks)))
}

private func paragraph(_ text: String) -> DocBlock {
  .text(kind: .paragraph, lines: [TextLine([spans(text)])])
}

private func thought(_ id: BlockID, _ text: String) -> Block {
  Block(id: id, turn: 1, kind: .thinking(text: text))
}

/// The 10 000-block transcript spike T37.37 timed (research.md §9.5.13), from
/// Rust-shaped spans: prose, code, prose, diff, tool card in rotation.
func largeFixture(_ count: Int) -> [Block] {
  (0..<count).map { index in
    let id = "b\(index)"
    switch index % 5 {
    case 0, 2:
      let sentences = (0..<(2 + index % 5)).flatMap { sentence in
        [
          spans("Block \(index) sentence \(sentence) explains "), spans("why", bold: true),
          spans(" the "), spans("turn_\(index)", token: .accent),
          spans(" step ran before the compaction and what the model saw next. "),
        ]
      }
      return reply(id, [.text(kind: .paragraph, lines: [TextLine(sentences)])])
    case 1:
      let lines = (0..<(3 + index % 9)).map { line in
        [spans("let value\(line) = compute(block: \(index), line: \(line)) // step \(line)")]
      }
      return reply(id, [.code(lang: "swift", lines: lines)])
    case 3:
      let lines = [
        [spans("@@ -\(index),3 +\(index),3 @@", token: .diffHunk)],
        [spans(" fn turn_\(index)() {")],
        [spans("-    let budget = \(index);", token: .diffDel)],
        [spans("+    let budget = \(index + 1);", token: .diffAdd)], [spans(" }")],
      ]
      return reply(id, [.code(lang: "diff", lines: lines)])
    default:
      return tool(id, "cargo nextest run -p cox-core -- turn_\(index) · exit 0")
    }
  }
}

/// Every character's font, colour and block spacing, to compare two views.
@MainActor
private func looks(_ view: TranscriptTextView) -> [String] {
  guard let storage = view.textStorage else { return [] }
  return (0..<storage.length).map { index in
    let font = storage.attribute(.font, at: index, effectiveRange: nil) as? NSFont
    let color = storage.attribute(.foregroundColor, at: index, effectiveRange: nil) as? NSColor
    let spacing =
      storage.attribute(.paragraphStyle, at: index, effectiveRange: nil) as? NSParagraphStyle
    return "\(font?.fontName ?? "-") \(color?.description ?? "-") \(spacing?.paragraphSpacing ?? 0)"
  }
}

@MainActor
private func expectSameText(_ patched: TranscriptTextView, as blocks: [Block]) {
  let loaded = TranscriptTextView.make(style: patched.style)
  loaded.openThoughts = patched.openThoughts
  loaded.load(blocks)
  #expect(patched.string == loaded.string)
  #expect(patched.blockRanges == loaded.blockRanges)
  #expect(looks(patched) == looks(loaded))
  #expect(patched.docStarts == loaded.docStarts)
}

/// Storage edits, as `NSTextStorage` reports them after each batch.
@MainActor
final class Edits {
  var ranges: [NSRange] = []
  private var token: (any NSObjectProtocol)?

  init(_ storage: NSTextStorage) {
    token = NotificationCenter.default.addObserver(
      forName: NSTextStorage.didProcessEditingNotification, object: storage, queue: nil
    ) { [weak self] note in
      let range = (note.object as? NSTextStorage)?.editedRange
      MainActor.assumeIsolated { range.map { self?.ranges.append($0) } }
    }
  }
}

@MainActor
@Suite(.serialized)
struct TranscriptPatchesTests {
  var style: TranscriptStyle {
    var style = TranscriptStyle.system
    (style.blockSpacing, style.colors) = (6, [.dim: .secondaryLabelColor, .accent: .systemBlue])
    return style
  }

  @Test(arguments: fixtures)
  func recordedPatchesGiveTheTextALoadGives(url: URL) throws {
    let fixture = try Fixture(contentsOf: url)
    let view = TranscriptTextView.make(style: style)
    var current: [BlockID: Block] = [:]
    for batch in fixture.batches {
      for case .upsert(let block, _) in batch { current[block.id] = block }
      view.apply(batch, current: { current[$0] })
    }
    expectSameText(view, as: fixture.snapshot)
    #expect(view.blocks == Dictionary(uniqueKeysWithValues: fixture.snapshot.map { ($0.id, $0) }))
  }

  @Test func blocksGainingAndLosingTextKeepOneSeparator() {
    let view = TranscriptTextView.make(style: style)
    // Open, so their reasoning is text that grows.
    view.openThoughts = ["t", "u", "x"]
    let meta = Block(
      id: "m", turn: 1, kind: .turnMeta(model: "x", tier: .code, usage: nil, stop: nil))
    view.load([meta, thought("t", ""), reply("r", [])])
    #expect(view.string.isEmpty)

    // The first text anywhere, then text after it and before it.
    view.apply([.docTail(id: "r", from: 0, blocks: [paragraph("Hello")])], current: { _ in nil })
    view.apply([.appendText(id: "t", text: "hm")], current: { _ in nil })
    view.apply([.upsert(block: thought("u", "first"), after: nil)], current: { _ in nil })
    expectSameText(
      view, as: [thought("u", "first"), meta, thought("t", "hm"), reply("r", [paragraph("Hello")])])

    // The reply grows, its tail is re-sent, a thought ends a line.
    view.apply(
      [
        .docTail(id: "r", from: 1, blocks: [.code(lang: "sh", lines: [[spans("cargo")]])]),
        .docTail(id: "r", from: 1, blocks: [.rule, paragraph("Done.")]),
        .appendText(id: "t", text: "m\nyes"),
      ], current: { _ in nil })
    let final = reply("r", [paragraph("Hello"), .rule, paragraph("Done.")])
    expectSameText(view, as: [thought("u", "first"), meta, thought("t", "hmm\nyes"), final])

    // A block between two others: its separator keeps the look of the one before.
    let mid = Block(id: "x", turn: 1, kind: .user(text: "mid", attachments: []))
    view.apply([.upsert(block: mid, after: "u")], current: { _ in nil })
    expectSameText(view, as: [thought("u", "first"), mid, meta, thought("t", "hmm\nyes"), final])
    view.apply([.remove(id: "t")], current: { _ in nil })
    expectSameText(view, as: [thought("u", "first"), mid, meta, final])

    // Losing text: the first block, then the last, then the only one.
    view.apply([.remove(id: "u"), .remove(id: "r")], current: { _ in nil })
    expectSameText(view, as: [mid, meta])
    view.apply([.upsert(block: thought("x", ""), after: nil)], current: { _ in nil })
    expectSameText(view, as: [thought("x", ""), meta])
    #expect(view.string.isEmpty)
  }

  @Test func streamedAppendTextEditsOnlyItsBlocksRange() throws {
    let view = TranscriptTextView.make(style: style)
    let blocks = [
      thought("a", "Before."), thought("t", "Thinking"), reply("r", [paragraph("After.")]),
    ]
    view.openThoughts = ["t"]
    view.load(blocks)
    let storage = try #require(view.textStorage)
    let edits = Edits(storage)
    let before = try #require(view.range(of: "a"))
    let after = try #require(view.range(of: "r"))

    view.apply([.appendText(id: "t", text: " harder")], current: { _ in nil })

    let block = try #require(view.range(of: "t"))
    #expect((view.string as NSString).substring(with: block) == "\u{FFFC}\nThinking harder")
    #expect(!edits.ranges.isEmpty)
    for edit in edits.ranges {
      #expect(NSIntersectionRange(edit, block) == edit, "edited \(edit) outside \(block)")
    }
    #expect(view.range(of: "a") == before)
    #expect(view.range(of: "r") == NSRange(location: after.location + 7, length: after.length))
    #expect((view.string as NSString).substring(with: view.range(of: "r") ?? block) == "After.")
  }

  @Test func docTailEditsOnlyItsTail() throws {
    let view = TranscriptTextView.make(style: style)
    view.load([reply("r", [paragraph("Frozen."), paragraph("Tai")]), thought("t", "Next.")])
    let storage = try #require(view.textStorage)
    let edits = Edits(storage)
    let frozen = try #require(view.range(of: "r")).location + ("Frozen." as NSString).length

    view.apply([.docTail(id: "r", from: 1, blocks: [paragraph("Tail.")])], current: { _ in nil })

    let block = try #require(view.range(of: "r"))
    #expect((view.string as NSString).substring(with: block) == "Frozen.\nTail.")
    for edit in edits.ranges {
      #expect(edit.location >= frozen && NSMaxRange(edit) <= NSMaxRange(block), "edited \(edit)")
    }
  }

  @Test func tenThousandBlocksBuiltPatchByPatchFitTheLaunchBudget() {
    let blocks = largeFixture(10_000)
    let byID = Dictionary(uniqueKeysWithValues: blocks.map { ($0.id, $0) })
    let view = TranscriptTextView.make(style: style)
    let screen = Offscreen(view, blocks: [])
    defer { screen.close() }

    let start = CACurrentMediaTime()
    var after: BlockID?
    // In batches of 64, the coalescer's cap (DT§4.3).
    for batch in stride(from: 0, to: blocks.count, by: 64) {
      let patches = blocks[batch..<min(batch + 64, blocks.count)].map { block in
        defer { after = block.id }
        return TimelinePatch.upsert(block: block, after: after)
      }
      view.apply(patches, current: { byID[$0] })
    }
    screen.flush()
    let firstFrame = (CACurrentMediaTime() - start) * 1_000

    #expect(view.blockRanges.count == 10_000)
    let budget = {
      #expect(
        firstFrame <= 400, "DT§1: an interactive window in 400 ms, took \(Int(firstFrame)) ms")
    }
    // DT§1 budgets are timed on a Mac; a CI VM only reports them.
    if ProcessInfo.processInfo.environment["CI"] == nil {
      budget()
    } else {
      withKnownIssue("DT§1 budgets are timed on a Mac, not a CI VM", isIntermittent: true, budget)
    }
  }
}
