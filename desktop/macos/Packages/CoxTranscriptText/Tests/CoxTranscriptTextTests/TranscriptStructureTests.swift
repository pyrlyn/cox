// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A reply's structure in the text (T37.23.8, A92): a heading in its level's font (A94)
// without its `#` run, a list item's marker in the gutter before its text, a quote
// line past a bar per quote in the quote's bar (A97), a table on tab stops, a rule as one character, and a
// streamed reply with the same paragraph styles a whole load gives it.

import AppKit
import CoxClient
import Testing

@testable import CoxTranscriptText

private func span(_ text: String, token: StyleToken = .text, bold: Bool = false) -> Span {
  var span = Span(text: text)
  (span.token, span.bold) = (token, bold)
  return span
}

/// A reply as `cox-render` sends it: level, depth and marker apart from the text.
private let structured: [DocBlock] = [
  .text(kind: .heading(2), lines: [TextLine([span("Plan", bold: true)])]),
  .text(
    kind: .list,
    lines: [TextLine([span("one")], marker: "•"), TextLine([span("two")], depth: 1, marker: "•")]),
  .text(kind: .quote, lines: [TextLine([span("quoted")], quote: 1)]),
  .text(kind: .quote, lines: [TextLine([span("deeper")], quote: 2)]),
  .rule,
  .table(rows: [["k", "value"], ["key", "v"]]),
]

@MainActor private let style: TranscriptStyle = {
  var style = TranscriptStyle.system
  style.headings = .init(
    h1: .preferredFont(forTextStyle: .title1), h3: .preferredFont(forTextStyle: .title3),
    h4: .preferredFont(forTextStyle: .headline))
  (style.indent, style.thought.indent) = (20, 12)
  return style
}()

private func reply(_ blocks: [DocBlock]) -> Block {
  Block(id: "a", turn: 1, kind: .assistant(text: "", doc: StyledDoc(blocks: blocks)))
}

@MainActor
private func paragraph(_ view: TranscriptTextView, at text: String) -> NSParagraphStyle? {
  let location = (view.string as NSString).range(of: text).location
  return view.textStorage?.attribute(.paragraphStyle, at: location, effectiveRange: nil)
    as? NSParagraphStyle
}

@MainActor
struct TranscriptStructureTests {
  @Test func markersSitInTheGutterQuotesPastTheirBarsAndHeadingsInTheirFont() throws {
    let view = TranscriptTextView.make(style: style)
    view.load([reply(structured)])

    #expect(view.string.hasPrefix("Plan\n"), "a heading shows without its `#` run")
    let font = view.textStorage?.attribute(.font, at: 0, effectiveRange: nil) as? NSFont
    #expect(font == style.headings.h3)
    let one = try #require(paragraph(view, at: "\t•\tone"))
    #expect(one.firstLineHeadIndent == 0 && one.headIndent == style.indent)
    #expect(one.tabStops.map(\.location).last == style.indent, "the text starts past the gutter")
    let two = try #require(paragraph(view, at: "\t•\ttwo"))
    #expect(two.firstLineHeadIndent == style.indent && two.headIndent == 2 * style.indent)
    let quote = try #require(paragraph(view, at: "quoted"))
    #expect(quote.firstLineHeadIndent == 12 && quote.headIndent == 12)
    #expect(try #require(paragraph(view, at: "deeper")).headIndent == 24, "a bar per quote")
    #expect(!view.string.contains("│") && !view.string.contains("#"))
    #expect(try #require(paragraph(view, at: "k\tvalue")).tabStops.count == 1)
    #expect(view.string.contains("\n\u{FFFC}\n"), "a rule is one character on its own line")
  }

  @Test func eachHeadingLevelTakesItsTokensFontAndLineHeight() throws {
    let levels = (1...6).map { level in
      DocBlock.text(kind: .heading(UInt8(level)), lines: [TextLine([span("Level \(level)")])])
    }
    var style = style
    style.lineHeights.heading = 1.5
    let view = TranscriptTextView.make(style: style)
    view.load([reply(levels)])
    let text = view.string as NSString
    let fonts = (1...6).map { level in
      view.textStorage?.attribute(
        .font, at: text.range(of: "Level \(level)").location, effectiveRange: nil) as? NSFont
    }
    let (h1, h3, h4) = (style.headings.h1, style.headings.h3, style.headings.h4)
    #expect(fonts == [h1, h3, h4, h4, h4, h4], "DT§5.9 sizes levels 1–3; 4–6 take h4")
    let spacing = { (font: NSFont) in
      TranscriptStyle.lines(font, style.lineHeights.heading).lineSpacing
    }
    #expect(try #require(paragraph(view, at: "Level 1")).lineSpacing == spacing(h1))
    #expect(try #require(paragraph(view, at: "Level 3")).lineSpacing == spacing(h4))
  }

  @Test func aQuotesBarsTakeTheQuoteStyleNotTheThoughtsRule() throws {
    var style = style
    style.quote = .init(bar: .systemRed, barWidth: 4)
    let view = TranscriptTextView.make(style: style)
    view.load([reply(structured)])
    let location = (view.string as NSString).range(of: "deeper").location
    let own =
      view.textStorage?.attribute(.transcriptParagraph, at: location, effectiveRange: nil)
      as? Paragraph
    let rails = try #require(own?.rails)
    #expect(rails.count == 2 && rails.step == style.thought.indent)
    #expect(rails.width == 4 && rails.color == .systemRed)
  }

  @Test func aStreamedReplyHasTheParagraphStylesAWholeLoadGivesIt() {
    let streamed = TranscriptTextView.make(style: style)
    streamed.load([reply(Array(structured.prefix(2)))])
    streamed.apply(
      [.docTail(id: "a", from: 1, blocks: Array(structured.dropFirst()))], current: { _ in nil })
    let loaded = TranscriptTextView.make(style: style)
    loaded.load([reply(structured)])

    #expect(streamed.string == loaded.string)
    let styles = { (view: TranscriptTextView) in
      (0..<(view.string as NSString).length).map {
        view.textStorage?.attribute(.paragraphStyle, at: $0, effectiveRange: nil)
          as? NSParagraphStyle
      }
    }
    #expect(styles(streamed) == styles(loaded))
  }

  @Test func aQuoteLineLaysOutWithItsBarsAndProseWithout() throws {
    let view = TranscriptTextView.make(style: style)
    view.load([reply(structured)])
    let manager = try #require(view.textLayoutManager)
    manager.ensureLayout(for: manager.documentRange)
    var quoted: [String] = []
    manager.enumerateTextLayoutFragments(from: manager.documentRange.location) { fragment in
      let text = (fragment.textElement as? NSTextParagraph)?.attributedString.string
      if fragment is QuoteFragment, let text {
        quoted.append(text.trimmingCharacters(in: .newlines))
      }
      return true
    }
    #expect(quoted == ["quoted", "deeper"])
  }
}
