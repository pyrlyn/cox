// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Headings and quotes at CoxUI's tokens, in light and dark, Solid, through the SwiftUI view the
// app hosts: every heading level at its token's size (T37.23.15, A94) and a quote nested three
// deep with a bar per depth in `quote.bar` (T37.23.17, A97). Apart from
// `TranscriptReplyTests`, whose one reply shows every block kind once, because these show one
// kind at each of its levels.

import AppKit
import CoxClient
import SnapshotTesting
import Testing

private func heading(_ level: UInt8) -> DocBlock {
  var span = Span(text: "Heading level \(level)")
  span.bold = true
  return .text(kind: .heading(level), lines: [TextLine([span])])
}

private func reply(_ blocks: [DocBlock]) -> [Block] {
  [Block(id: "a", turn: 1, kind: .assistant(text: "", doc: StyledDoc(blocks: blocks)))]
}

/// Levels 1 to 6, each followed by prose at the body size to read it against.
private let headings = reply(
  (1...6).flatMap { level in
    [heading(UInt8(level)), .text(kind: .paragraph, lines: [TextLine([Span(text: "Body text.")])])]
  })

/// A quote three deep, its first line long enough to wrap, so the bars run down both lines.
private let quote = reply([
  .text(
    kind: .quote,
    lines: [
      TextLine(
        [
          Span(
            text: "The watcher waits for its first event, so a slow disk no longer fails the "
              + "test and the retry loop that hid the race is gone.")
        ], quote: 1),
      TextLine([Span(text: "nested once")], quote: 2),
      TextLine([Span(text: "nested twice")], quote: 3),
      TextLine([Span(text: "back out")], quote: 1),
    ]),
  .text(kind: .paragraph, lines: [TextLine([Span(text: "After the quote.")])]),
])

// The reading column's width; the height follows the text. The quote's is narrower, so its
// bars are enough of the image that drawing them in the hairline fails the snapshot.
// swiftlint:disable:next no_literal_size
private let size = NSSize(width: 760, height: 400)
// swiftlint:disable:next no_literal_size
private let quoteSize = NSSize(width: 320, height: 200)

@MainActor
@Suite(.serialized)
struct TranscriptHeadingQuoteTests {
  @Test(arguments: [false, true])
  func everyHeadingLevel(dark: Bool) throws {
    let host = Host(headings, size: size, dark: dark)
    defer { host.close() }
    host.fitToText()
    // Anti-aliasing differs slightly between machines; a real change moves far more pixels.
    assertSnapshot(
      of: try host.image(), as: .image(precision: 0.995, perceptualPrecision: 0.98),
      named: dark ? "dark-solid" : "light-solid", testName: "everyHeadingLevel")
  }

  @Test(arguments: [false, true])
  func nestedQuote(dark: Bool) throws {
    let host = Host(quote, size: quoteSize, dark: dark)
    defer { host.close() }
    host.fitToText()
    assertSnapshot(
      of: try host.image(), as: .image(precision: 0.995, perceptualPrecision: 0.98),
      named: dark ? "dark-solid" : "light-solid", testName: "nestedQuote")
  }
}
