// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A prompt's bubble and a thought's fold as text (T37.23.4): a thought opens
// and folds by editing only the text after its header, a prompt's attachments
// are one small attachment each, both copy as the text a reader sees, and their
// paragraphs are laid out by the fragment that draws behind them.

import AppKit
import CoxClient
import Foundation
import Testing

@testable import CoxTranscriptText

private func notice(_ id: BlockID, _ text: String) -> Block {
  Block(id: id, turn: 1, kind: .notice(level: .info, text: text))
}

private let reasoning = "The watcher fires late.\nWait for it."

@MainActor
@Suite(.serialized)
struct TranscriptDecorTests {
  @Test func openingAndFoldingAThoughtEditsOnlyItsReasoning() throws {
    let view = TranscriptTextView.make()
    view.load([
      notice("a", "Before."), Block(id: "k", turn: 1, kind: .thinking(text: reasoning)),
      notice("b", "After."),
    ])
    let folded = view.string
    let header = try #require(view.range(of: "k"))
    let after = try #require(view.range(of: "b"))
    #expect(header.length == 1, "folded, a thought is its header")
    let storage = try #require(view.textStorage)
    let edits = Edits(storage)

    // The header's button asks on the next run-loop turn, after its click.
    view.hostedCards.toggle("k")
    RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.05))

    let open = try #require(view.range(of: "k"))
    #expect((view.string as NSString).substring(with: open) == "\u{FFFC}\n" + reasoning)
    #expect(view.range(of: "a") == NSRange(location: 0, length: 7))
    #expect(view.range(of: "b")?.location == after.location + open.length - 1)
    // The thought and the line break that ends its last paragraph.
    let thought = NSRange(location: open.location, length: open.length + 1)
    for edit in edits.ranges {
      #expect(NSIntersectionRange(edit, thought) == edit, "edited \(edit) outside \(thought)")
    }
    let attachment =
      storage.attribute(.attachment, at: open.location, effectiveRange: nil)
      as? CardAttachment
    #expect(attachment != nil)

    view.setThought("k", open: false)
    #expect(view.string == folded)
    #expect(view.range(of: "b") == after)
    let kept = storage.attribute(.attachment, at: header.location, effectiveRange: nil)
    #expect(kept as? CardAttachment === attachment, "the header keeps its view")
  }

  @Test func aPromptsAttachmentsAreATileEachAndCopyAsItsText() throws {
    let prompt = "Why does this fail?\nSee the log."
    let view = TranscriptTextView.make()
    view.load([
      Block(id: "u", turn: 1, kind: .user(text: prompt, attachments: ["run.log", "shot.png"]))
    ])
    let range = try #require(view.range(of: "u"))
    let storage = try #require(view.textStorage)

    #expect(range.length == (prompt as NSString).length + 3, "the text, a line break, two tiles")
    for offset in [range.length - 2, range.length - 1] {
      #expect(storage.attribute(.attachment, at: offset, effectiveRange: nil) is CardAttachment)
    }
    view.setSelectedRange(range)
    #expect(view.copiedSelection().markdown == prompt)
    #expect(view.copiedSelection().plain == prompt)
  }

  @Test func aDecoratedBlocksParagraphsKnowTheirEdgesAndDrawBehindTheirText() throws {
    let view = TranscriptTextView.make()
    view.openThoughts = ["k"]
    let screen = Offscreen(
      view,
      blocks: [
        Block(id: "u", turn: 1, kind: .user(text: "One\nTwo\nThree", attachments: [])),
        Block(id: "k", turn: 1, kind: .thinking(text: reasoning)), notice("n", "Plain."),
      ])
    defer { screen.close() }
    let storage = try #require(view.textStorage)
    let prompt = try #require(view.range(of: "u"))
    func edge(_ location: Int) -> Int? {
      storage.attribute(.transcriptEdge, at: location, effectiveRange: nil) as? Int
    }

    #expect([0, 4, 8].map { edge(prompt.location + $0) } == [1, 0, 2])
    let thought = try #require(view.range(of: "k"))
    #expect(edge(thought.location) == 1, "the header opens the thought")
    #expect(edge(NSMaxRange(thought) - 1) == 2)
    #expect(screen.frame(of: "u") != nil)
    let manager = try #require(view.textLayoutManager)
    for (id, decorated) in [("u", true), ("k", true), ("n", false)] {
      let start = try #require(view.range(of: id).flatMap(view.textRange)?.location)
      let fragment = manager.textLayoutFragment(for: start)
      #expect((fragment is DecorFragment) == decorated, "\(id)")
    }
  }

  @Test func aThoughtsHeaderReadsThinkingUntilItsDurationArrives() throws {
    let streaming = Block(id: "k", turn: 1, kind: .thinking(text: reasoning))
    #expect(TranscriptCards.thoughtTitle(streaming.kind) == "Thinking")
    let view = TranscriptTextView.make()
    view.load([streaming])
    var done = streaming
    done.kind = .thinking(text: reasoning, durationMs: 12_400)
    view.apply([.upsert(block: done, after: nil)]) { _ in done }

    let storage = try #require(view.textStorage)
    let start = try #require(view.range(of: "k")).location
    let header = try #require(
      storage.attribute(.attachment, at: start, effectiveRange: nil) as? CardAttachment)
    #expect(TranscriptCards.thoughtTitle(header.block.kind) == "Thought for 12 s")
    #expect(TranscriptCards.thoughtTitle(.thinking(text: "", durationMs: 90)) == "Thought for 1 s")
  }

  @Test func aPromptsTileRowSitsAGapBelowItsText() throws {
    var style = TranscriptStyle.system
    // swiftlint:disable:next no_literal_radius no_literal_size
    style.bubble = .init(fill: .gray, radius: 8, padding: NSSize(width: 12, height: 10), gap: 8)
    let view = TranscriptTextView.make(style: style)
    view.load([
      Block(id: "u", turn: 1, kind: .user(text: "One\nTwo", attachments: ["a.log"]))
    ])
    let storage = try #require(view.textStorage)
    func spacingBefore(_ location: Int) -> CGFloat? {
      (storage.attribute(.paragraphStyle, at: location, effectiveRange: nil) as? NSParagraphStyle)?
        .paragraphSpacingBefore
    }
    #expect(spacingBefore(0) == 10, "the first line sits the padding below the top")
    #expect(spacingBefore(4) == 0, "a later line of text follows on")
    #expect(spacingBefore(8) == 8, "the tile row sits the gap below the text")
  }
}
