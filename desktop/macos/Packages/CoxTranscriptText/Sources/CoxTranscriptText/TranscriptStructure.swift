// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A reply's structure in the transcript text (T37.23.8, DT§5.2): headings, list
// items, quote lines, tables and rules stay text in the one text (A87), set by
// paragraph styles rather than drawn as flat paragraphs. `cox-render` sends a
// heading's level, a quote's depth and a list item's marker apart from the text
// (A92): a heading shows at its level's size (A94) without its `#` run, a quote
// line sits past a bar per quote (`QuoteFragment`, in the quote's own bar, A97),
// and an item's marker hangs in the gutter before its text. Its own file because
// it is the one place a doc block's kind becomes layout.

import AppKit
import CoxClient

extension NSAttributedString.Key {
  /// A doc line's `Paragraph`, on its first character.
  static let transcriptParagraph = NSAttributedString.Key("cox.transcript.paragraph")
}

extension TextLine {
  /// What the text holds in front of the line's spans: an item's marker between
  /// two tabs, so it sits right-aligned in the gutter and the text starts past it.
  var lead: String { marker.isEmpty ? "" : "\t\(marker)\t" }
}

extension TranscriptStyle {
  /// A reply heading's font at each of DS§3.2's heading tokens (A94).
  public struct Headings: Equatable {
    public var h1: NSFont
    public var h3: NSFont
    public var h4: NSFont

    public init(h1: NSFont, h3: NSFont, h4: NSFont) {
      (self.h1, self.h3, self.h4) = (h1, h3, h4)
    }

    /// The same font at every level.
    public init(_ font: NSFont) { self.init(h1: font, h3: font, h4: font) }

    func font(_ size: HeadingSize) -> NSFont {
      switch size {
      case .h1: h1
      case .h3: h3
      case .h4: h4
      }
    }
  }

  /// A quote's bars (A97): one per depth, a thought's indent apart.
  public struct Quote: Equatable {
    public var bar: NSColor
    public var barWidth: CGFloat

    public init(bar: NSColor, barWidth: CGFloat) {
      (self.bar, self.barWidth) = (bar, barWidth)
    }

    public static var system: Quote { Quote(bar: .tertiaryLabelColor, barWidth: 3) }
  }

  /// The heading token a Markdown heading's level takes: DT§5.9 sizes headings 17 / 15 / 13 pt,
  /// levels 1, 2 and 3. It does not size levels 4–6; they take `h4` too, so no deeper
  /// heading is larger than a shallower one (A94).
  enum HeadingSize: Int, CaseIterable {
    case h1, h3, h4

    init(level: UInt8) {
      self =
        switch level {
        case 1: .h1
        case 2: .h3
        default: .h4
        }
    }
  }
}

extension TextLook {
  /// A span's face: a heading's at its level's size.
  enum Face: Equatable {
    case body, code
    case heading(TranscriptStyle.HeadingSize)

    /// Its four fonts' place in `fonts`.
    var index: Int {
      switch self {
      case .body: 0
      case .code: 1
      case .heading(let size): 2 + size.rawValue
      }
    }
  }
}

/// A doc line's own paragraph style, the same with the block spacing for when it
/// is its block's last paragraph (`TranscriptText.respace`), and its quote bars.
final class Paragraph: NSObject {
  /// A bar per quote, `step` apart from the text column's edge, in the style's quote bar.
  struct Rails {
    let count: Int
    let step: CGFloat
    let width: CGFloat
    let color: NSColor
  }

  let own: NSParagraphStyle
  let last: NSParagraphStyle
  let rails: Rails?

  init(_ own: NSMutableParagraphStyle, spacing: CGFloat, rails: Rails? = nil) {
    let last = own.mutableCopy() as? NSMutableParagraphStyle ?? NSMutableParagraphStyle()
    last.paragraphSpacing += spacing
    (self.own, self.last, self.rails) = (own, last, rails)
  }
}

/// A style's doc paragraphs, each made once per shape of line. A table's tab
/// stops follow its cells, so each table makes its own.
final class Paragraphs {
  private struct Shape: Hashable {
    let heading: TranscriptStyle.HeadingSize?
    let quote: UInt8
    /// A list line's depth; `nil` outside a list.
    let depth: UInt8?
    /// Whether the line starts an item, its marker first.
    let marked: Bool
  }

  private let style: TranscriptStyle
  /// A heading's, per `HeadingSize`.
  private let headings: [Paragraph]
  /// A code block's lines, at the code face's line height.
  let code: Paragraph
  /// Between an item's marker and its text: a space of body text.
  private let gap: CGFloat
  private var made: [Shape: Paragraph] = [:]

  init(_ style: TranscriptStyle) {
    self.style = style
    headings = TranscriptStyle.HeadingSize.allCases.map { size in
      let heading = TranscriptStyle.lines(style.headings.font(size), style.lineHeights.heading)
      heading.paragraphSpacingBefore = style.blockSpacing
      return Paragraph(heading, spacing: style.blockSpacing)
    }
    code = Paragraph(
      TranscriptStyle.lines(style.code, style.lineHeights.code), spacing: style.blockSpacing)
    gap = (" " as NSString).size(withAttributes: [.font: style.body]).width.rounded(.up)
  }

  /// A text line's paragraph: a heading's; a quote line's past its bars, a
  /// thought's indent each; a list line's past its depth's indents, the list's
  /// own indent the gutter its marker sits in. `nil` for prose.
  func of(_ kind: TextKind, _ line: TextLine) -> Paragraph? {
    let heading: TranscriptStyle.HeadingSize? =
      if case .heading(let level) = kind { .init(level: level) } else { nil }
    let depth = kind == .list ? line.depth : nil
    let marked = depth != nil && !line.marker.isEmpty
    guard heading != nil || depth != nil || line.quote > 0 else { return nil }
    if let heading, line.quote == 0 { return headings[heading.rawValue] }
    let shape = Shape(heading: heading, quote: line.quote, depth: depth, marked: marked)
    if let paragraph = made[shape] { return paragraph }
    let paragraph =
      heading.map {
        headings[$0.rawValue].own.mutableCopy() as? NSMutableParagraphStyle
          ?? NSMutableParagraphStyle()
      } ?? TranscriptStyle.lines(style.body, style.lineHeights.body)
    let base = CGFloat(line.quote) * style.thought.indent
    (paragraph.firstLineHeadIndent, paragraph.headIndent) = (base, base)
    if let depth {
      let text = base + style.indent * CGFloat(Int(depth) + 1)
      paragraph.firstLineHeadIndent = marked ? text - style.indent : text
      paragraph.headIndent = text
      if marked {
        let marker = max(paragraph.firstLineHeadIndent, text - gap)
        paragraph.tabStops = [
          NSTextTab(textAlignment: .right, location: marker),
          NSTextTab(textAlignment: .left, location: text),
        ]
      }
    }
    let rails =
      line.quote > 0
      ? Paragraph.Rails(
        count: Int(line.quote), step: style.thought.indent, width: style.quote.barWidth,
        color: style.quote.bar)
      : nil
    let made = Paragraph(paragraph, spacing: style.blockSpacing, rails: rails)
    self.made[shape] = made
    return made
  }

  /// A table's rows, their cells on tab stops past each column's widest cell.
  func table(_ rows: [[String]]) -> Paragraph {
    var widths: [CGFloat] = []
    for row in rows {
      for (column, cell) in row.enumerated() {
        let width = (cell as NSString).size(withAttributes: [.font: style.body]).width
        if column < widths.count {
          widths[column] = max(widths[column], width)
        } else {
          widths.append(width)
        }
      }
    }
    let paragraph = TranscriptStyle.lines(style.body, style.lineHeights.body)
    var location: CGFloat = 0
    paragraph.tabStops = widths.dropLast().map { width in
      location += (width + style.indent).rounded(.up)
      return NSTextTab(textAlignment: .left, location: location)
    }
    return Paragraph(paragraph, spacing: style.blockSpacing)
  }
}

/// A doc rule: a thought's hairline across the text column, as one
/// attachment character, so it stays in the text and a drag runs over it.
final class RuleAttachment: NSTextAttachment {
  static let mark = "\u{FFFC}"
  private let thickness: CGFloat

  init(_ thought: TranscriptStyle.Thought) {
    thickness = thought.ruleWidth
    super.init(data: nil, ofType: nil)
    // Drawn at its bounds' size each time its line is, so the colour resolves in the view's
    // appearance. TextKit 2 draws an attachment's `image`, not `image(for:)`, without a view.
    let size = NSSize(width: thickness, height: thickness)
    image = NSImage(size: size, flipped: false) { [color = thought.rule] in
      color.setFill()
      $0.fill()
      return true
    }
  }

  required init?(coder: NSCoder) { nil }

  /// The full line width, at the middle of the line's lowercase letters.
  override func attachmentBounds(
    for attributes: [NSAttributedString.Key: Any], location: any NSTextLocation,
    textContainer: NSTextContainer?, proposedLineFragment: CGRect, position: CGPoint
  ) -> CGRect {
    let padding = textContainer?.lineFragmentPadding ?? 0
    let middle = ((attributes[.font] as? NSFont)?.xHeight ?? 0) / 2
    return CGRect(
      x: 0, y: middle, width: max(0, proposedLineFragment.width - 2 * padding), height: thickness)
  }
}

/// A quote line's layout: its bars at the text column's edge, under its text.
final class QuoteFragment: NSTextLayoutFragment {
  /// Whether `text`, a paragraph's, is a quote line's.
  static func quoted(_ text: NSAttributedString?) -> Bool {
    rails(text) != nil
  }

  private static func rails(_ text: NSAttributedString?) -> Paragraph.Rails? {
    guard let text, text.length > 0 else { return nil }
    return (text.attribute(.transcriptParagraph, at: 0, effectiveRange: nil) as? Paragraph)?.rails
  }

  /// Each bar in the fragment's coordinates: down its lines, and on through the
  /// space under them unless that space ends its block.
  private var bars: [(CGRect, NSColor)] {
    let text = (textElement as? NSTextParagraph)?.attributedString
    guard let rails = Self.rails(text), let text,
      let last = textLineFragments.last?.typographicBounds
    else { return [] }
    let frame = layoutFragmentFrame
    // The frame starts past the paragraph's indent; the bars start at the text column's edge.
    let margin = (textLayoutManager?.textContainer?.lineFragmentPadding ?? 0) - frame.minX
    let style = text.attribute(.paragraphStyle, at: 0, effectiveRange: nil) as? NSParagraphStyle
    let bottom = (style?.paragraphSpacing ?? 0) > 0 ? last.maxY : frame.height
    return (0..<rails.count).map { index in
      let x = margin + CGFloat(index) * rails.step
      return (CGRect(x: x, y: 0, width: rails.width, height: bottom), rails.color)
    }
  }

  override var renderingSurfaceBounds: CGRect {
    bars.reduce(super.renderingSurfaceBounds) { $0.union($1.0) }
  }

  override func draw(at point: CGPoint, in context: CGContext) {
    for (bar, color) in bars {
      context.setFillColor(color.cgColor)
      context.fill(bar.offsetBy(dx: point.x, dy: point.y))
    }
    super.draw(at: point, in: context)
  }
}

extension TranscriptText {
  /// `respace` for a block with structure: each paragraph in `spaced` takes
  /// its line's own style, the one at `last` its spaced one, and only one
  /// whose style is wrong changes.
  static func restyle(
    _ text: NSMutableAttributedString, _ spaced: NSRange, last: Int, _ look: TextLook
  ) {
    let string = text.mutableString
    var location = spaced.location
    while location < NSMaxRange(spaced) {
      let paragraph = string.paragraphRange(for: NSRange(location: location, length: 0))
      let own =
        text.attribute(.transcriptParagraph, at: paragraph.location, effectiveRange: nil)
        as? Paragraph
      let want = paragraph.location == last ? own?.last ?? look.spacing : own?.own ?? look.line
      text.enumerateAttribute(.paragraphStyle, in: paragraph) { value, range, _ in
        guard (value as? NSParagraphStyle) != want else { return }
        text.addAttribute(.paragraphStyle, value: want, range: range)
      }
      location = NSMaxRange(paragraph)
    }
  }
}
