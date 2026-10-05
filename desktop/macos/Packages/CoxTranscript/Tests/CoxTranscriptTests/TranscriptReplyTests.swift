// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A reply's structure (T37.23.8's and T37.23.12's Check): one reply with every doc
// block kind as `cox-render` sends it — headings, prose, a bulleted and a numbered
// list with a nested item, a nested quote, a rule, a table and code — in light and
// dark, Solid, through the SwiftUI view the app hosts: headings without their `#`
// run, markers in the gutter, a bar per quote. Copy as Markdown of it, whole and in
// part, asks the doc writer for the blocks it covers (the core writes the Markdown, T58.4.28;
// here the plain stand-in), and a loaded reply gives its source.

import AppKit
import CoxClient
import SnapshotTesting
import Testing

private func span(_ text: String, token: StyleToken = .text, bold: Bool = false) -> Span {
  var span = Span(text: text)
  (span.token, span.bold) = (token, bold)
  return span
}

private func heading(_ level: UInt8, _ text: String) -> DocBlock {
  .text(kind: .heading(level), lines: [TextLine([span(text, bold: true)])])
}

private let wraps =
  "The watcher waits for its first event, so a slow disk no longer fails the test and the retry "
  + "loop that hid the race is gone."

/// Level, depth and markers apart from the text, as `cox-render` sends them (A92).
private let doc = StyledDoc(blocks: [
  heading(1, "Release notes"),
  .text(kind: .paragraph, lines: [TextLine([span("The fix is small.")])]),
  heading(2, "What changed"),
  .text(
    kind: .list,
    lines: [
      TextLine([span(wraps)], marker: "•"), TextLine([span("nested item")], depth: 1, marker: "•"),
    ]),
  .text(
    kind: .list,
    lines: [TextLine([span("first")], marker: "1."), TextLine([span("second")], marker: "2.")]),
  heading(3, "Why"),
  .text(
    kind: .quote,
    lines: [
      TextLine([span("Flaky no more. " + wraps)], quote: 1),
      TextLine([span("nested quote")], quote: 2),
    ]),
  .rule,
  .table(rows: [["Crate", "Tests"], ["cox-core", "412"], ["cox-tui", "88"]]),
  .code(lang: "rust", lines: [[span("let event = rx.recv().await?;")]]),
])

private let markdown = """
  # Release notes

  The fix is small.

  ## What changed

  - \(wraps)
    - nested item

  1. first
  2. second

  ### Why

  > Flaky no more. \(wraps)
  > > nested quote

  ---

  | Crate | Tests |
  | --- | --- |
  | cox-core | 412 |
  | cox-tui | 88 |

  ```rust
  let event = rx.recv().await?;
  ```
  """

/// Streamed, so the reply has no source and copies from its doc.
private let reply = [Block(id: "a", turn: 1, kind: .assistant(text: "", doc: doc))]

// The reading column's width; the height follows the text.
// swiftlint:disable:next no_literal_size
private let size = NSSize(width: 760, height: 400)

@MainActor
@Suite(.serialized)
struct TranscriptReplyTests {
  @Test(arguments: [false, true])
  func replyWithEveryBlockKind(dark: Bool) throws {
    let host = Host(reply, size: size, dark: dark)
    defer { host.close() }
    host.fitToText()
    // Anti-aliasing differs slightly between machines; a real change moves far more pixels.
    assertSnapshot(
      of: try host.image(), as: .image(precision: 0.995, perceptualPrecision: 0.98),
      named: dark ? "dark-solid" : "light-solid", testName: "replyWithEveryBlockKind")
  }

  @Test func copyAsMarkdownAsksTheWriterForTheBlocksItCovers() throws {
    let host = Host(reply, size: size)
    defer { host.close() }
    let text = host.text.string as NSString

    host.text.setSelectedRange(NSRange(location: 0, length: text.length))
    let whole = host.copy()
    #expect(whole.markdown == PlainDocWriter().markdown(doc))
    #expect(whole.plain?.contains("\u{FFFC}") == false, "a rule copies as no text")

    // From the first list's bullet to the quote's end: whole doc blocks copy as Markdown.
    let start = text.range(of: "\t•\tThe").location
    let end = NSMaxRange(text.range(of: "nested quote"))
    host.text.setSelectedRange(NSRange(start..<end))
    let part = try #require(host.copy().markdown)
    // The list, the numbered list, "Why" and the quote.
    #expect(part == PlainDocWriter().markdown(StyledDoc(blocks: Array(doc.blocks[3...6]))))
  }

  @Test func copyAsMarkdownOfALoadedReplyGivesItsSource() throws {
    // `*` bullets, which the doc's own Markdown would give as `-`: the source is what copies.
    let source = markdown.replacing("- ", with: "* ")
    let loaded = [Block(id: "a", turn: 1, kind: .assistant(text: source, doc: doc))]
    let host = Host(loaded, size: size)
    defer { host.close() }
    host.text.setSelectedRange(NSRange(location: 0, length: (host.text.string as NSString).length))
    #expect(host.copy().markdown == source)
  }
}
