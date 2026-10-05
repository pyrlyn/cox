// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// TranscriptTextView against the recorded fixtures (T37.40's Check): every
// block id maps to its range and every location back to its block.

import AppKit
import CoxClient
import Foundation
import Testing

@testable import CoxTranscriptText

/// Every `desktop/macos/Fixtures/*.json`, found from this file so the tests
/// read the recorder's output in place.
let fixtures: [URL] = {
  let dir = URL(filePath: #filePath)
    .deletingLastPathComponent()  // CoxTranscriptTextTests
    .deletingLastPathComponent()  // Tests
    .deletingLastPathComponent()  // CoxTranscriptText
    .deletingLastPathComponent()  // Packages
    .deletingLastPathComponent()  // macos
    .appending(path: "Fixtures")
  let names = (try? FileManager.default.contentsOfDirectory(atPath: dir.path())) ?? []
  return names.filter { $0.hasSuffix(".json") }.sorted().map { dir.appending(path: $0) }
}()

/// A block that is only its text.
private func thinking(_ id: BlockID, _ text: String) -> Block {
  Block(id: id, turn: 1, kind: .notice(level: .info, text: text))
}

@Test func fixturesAreFound() {
  #expect(!fixtures.isEmpty)
}

@MainActor
@Test(arguments: fixtures)
func everyFixtureBlockMapsToItsRangeAndBack(url: URL) throws {
  let blocks = try Fixture(contentsOf: url).snapshot
  let view = TranscriptTextView.make()
  let scroll = view.inScrollView(frame: .zero)
  view.load(blocks)
  scroll.layoutSubtreeIfNeeded()
  let text = view.string as NSString

  #expect(view.textLayoutManager != nil, "stays on TextKit 2")
  #expect(view.blockRanges.ids == blocks.map(\.id))
  var end = 0
  for block in blocks {
    let range = try #require(view.range(of: block.id))
    #expect(range.location >= end, "ranges keep block order and never overlap")
    end = NSMaxRange(range)
    guard range.length > 0 else {
      #expect(view.blockID(at: range.location) != block.id, "a block without text holds no caret")
      continue
    }
    #expect(view.blockID(at: range.location) == block.id)
    #expect(view.blockID(at: NSMaxRange(range)) == block.id)
    if case .user(let said, _) = block.kind { #expect(text.substring(with: range) == said) }
  }
  #expect(end == text.length)
  for location in 0...text.length {
    let id = try #require(view.blockID(at: location), "location \(location) has a block")
    let range = try #require(view.range(of: id))
    #expect(range.location <= location && location <= NSMaxRange(range))
  }
  #expect(view.blockID(at: text.length + 1) == nil)
}

@MainActor
@Test func blockWithoutTextTakesNoLineAndNoCaret() {
  let view = TranscriptTextView.make()
  view.load([thinking("a", "one"), thinking("empty", ""), thinking("b", "two")])

  #expect(view.string == "one\ntwo")
  #expect(view.range(of: "empty") == NSRange(location: 3, length: 0))
  #expect(view.blockID(at: 3) == "a")
  #expect(view.blockID(at: 4) == "b")
}

@MainActor
@Test func repeatedIdKeepsItsFirstBlock() {
  let view = TranscriptTextView.make()
  view.load([thinking("a", "one"), thinking("a", "again")])

  #expect(view.string == "one")
  #expect(view.blockRanges.ids == ["a"])
}

@MainActor
@Test func codeLinesTakeTheCodeFont() throws {
  let style = TranscriptStyle.system
  let doc = StyledDoc(blocks: [
    .text(kind: .paragraph, lines: [TextLine([Span(text: "Run:")])]),
    .code(lang: "sh", lines: [[Span(text: "cargo")], [Span(text: "test")]]),
  ])
  let view = TranscriptTextView.make(style: style)
  view.load([Block(id: "m", turn: 1, kind: .assistant(text: "", doc: doc)), thinking("t", "x")])
  let storage = try #require(view.textStorage)

  #expect(view.string == "Run:\ncargo\ntest\nx")
  #expect(storage.attribute(.font, at: 0, effectiveRange: nil) as? NSFont == style.body)
  #expect(storage.attribute(.font, at: 5, effectiveRange: nil) as? NSFont == style.code)
  #expect(view.blockID(at: 13) == "m")
}

@Test func emptyTranscriptHasNoBlockAnywhere() {
  #expect(BlockRanges().index(at: 0) == nil)
  #expect(BlockRanges().index(at: -1) == nil)
}

@MainActor
@Test func linesStayInTheReadingColumnCentredAsTheViewWidens() throws {
  let edge: CGFloat = 16
  let column: CGFloat = 760
  let (tall, wide): (CGFloat, CGFloat) = (400, 1000)
  var style = TranscriptStyle.system
  (style.inset, style.readingWidth) = (NSSize(width: edge, height: edge), column)
  let view = TranscriptTextView.make(style: style)
  let scroll = view.inScrollView(
    frame: NSRect(origin: .zero, size: NSSize(width: wide, height: tall)))
  let padding = try #require(view.textContainer?.lineFragmentPadding)
  let lines = { (view.textContainer?.size.width ?? 0) - 2 * padding }
  #expect(lines() == column)
  // A legacy scroller (a Mac without a trackpad, as CI runners are) takes its width from the view.
  let visible = scroll.contentSize.width
  #expect(view.textContainerInset == NSSize(width: (visible - column) / 2 - padding, height: edge))
  scroll.setFrameSize(NSSize(width: wide * 2, height: tall))
  #expect(lines() == column)
  // Narrower than the column plus the insets, the lines take what the insets leave.
  scroll.setFrameSize(NSSize(width: column, height: tall))
  #expect(view.textContainerInset.width == edge)
}
