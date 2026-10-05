// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The transcript as one attributed string (T37.40): timeline blocks → text and
// the `BlockRanges` over it. Each block becomes one piece — a reply from the
// Rust `StyledDoc` spans (T37.43), a card as one attachment (T37.41), a prompt
// or a thought as decorated text (T37.23.4), anything else as plain runs;
// pieces are joined and tracked here only, and patches splice pieces into the
// text (`TranscriptPatches.swift`) with the same rules.

import AppKit
import CoxClient

/// What the transcript text is drawn with. This package holds no design
/// values: the app builds a style from CoxUI's tokens (T37.23), and `system`
/// is the platform's own body text for tests and previews.
public struct TranscriptStyle: Equatable {
  public var body: NSFont
  public var code: NSFont
  /// A reply heading's, one per DS§3.2 heading token (A94).
  public var headings: Headings
  /// The `.text` token's colour, and any token `colors` leaves out.
  public var text: NSColor
  /// Each Rust style token's colour (`StyleToken`, T37.7).
  public var colors: [StyleToken: NSColor]
  /// The space after a block's last line.
  public var blockSpacing: CGFloat
  /// The space between the text and the view's edges.
  public var inset: NSSize
  /// The widest a line runs, centred in a wider view as the composer is (DS§4); `nil`: no limit.
  public var readingWidth: CGFloat?
  /// A user prompt's bubble and a thought's look (`TranscriptDecor.swift`); a quote's bars.
  public var bubble: Bubble
  public var thought: Thought
  public var quote: Quote
  /// Where a prompt's turn number sits; `nil` draws none.
  public var gutter: Gutter?
  /// A list's indent, and the gap between a table's columns (`TranscriptStructure.swift`).
  public var indent: CGFloat
  /// Each face's line height (`TranscriptLineHeights.swift`).
  public var lineHeights: LineHeights

  public init(
    body: NSFont, code: NSFont, headings: Headings? = nil, text: NSColor,
    colors: [StyleToken: NSColor] = [:], blockSpacing: CGFloat, inset: NSSize,
    bubble: Bubble = .system, thought: Thought = .system, quote: Quote = .system,
    indent: CGFloat = 0, lineHeights: LineHeights = .natural, readingWidth: CGFloat? = nil,
    gutter: Gutter? = nil
  ) {
    (self.body, self.code, self.text, self.colors) = (body, code, text, colors)
    self.headings =
      headings ?? Headings(NSFontManager.shared.convert(body, toHaveTrait: .boldFontMask))
    (self.blockSpacing, self.inset, self.bubble, self.thought) = (
      blockSpacing, inset, bubble, thought
    )
    (self.quote, self.indent) = (quote, indent)
    (self.lineHeights, self.readingWidth, self.gutter) = (lineHeights, readingWidth, gutter)
  }

  public static var system: TranscriptStyle {
    let body = NSFont.preferredFont(forTextStyle: .body)
    let code =
      body.fontDescriptor.withDesign(.monospaced)
      .flatMap { NSFont(descriptor: $0, size: body.pointSize) } ?? body
    return TranscriptStyle(body: body, code: code, text: .textColor, blockSpacing: 0, inset: .zero)
  }

  public func color(_ token: StyleToken) -> NSColor { colors[token] ?? text }
}

/// A style's fonts and colours, with the bold and italic fonts made once per
/// build or batch rather than once per span.
struct TextLook {
  /// One run's look: the font and colour every character has, and a span's
  /// rarer attributes (strike, underline, link).
  struct Look {
    let font: NSFont
    let color: NSColor
    var extra: [NSAttributedString.Key: Any] = [:]

    var attributes: [NSAttributedString.Key: Any] {
      extra.merging([.font: font, .foregroundColor: color]) { _, new in new }
    }

    func same(as other: Look) -> Bool {
      font === other.font && color === other.color && extra.isEmpty && other.extra.isEmpty
    }
  }

  let style: TranscriptStyle
  /// A block's last paragraph carries this (`TranscriptText.respace`), every other prose
  /// paragraph `line`: both space the lines of body text as its line height says.
  let spacing: NSParagraphStyle
  let line: NSParagraphStyle
  /// A prompt's and a thought's look, one `Decor` each (`TranscriptDecor.swift`).
  let prompt: [NSAttributedString.Key: Any]
  let thought: [NSAttributedString.Key: Any]
  /// A reply's paragraph styles and its rule (`TranscriptStructure.swift`).
  let paragraphs: Paragraphs
  let rule: Look
  private let fonts: [NSFont]
  private let colors: [StyleToken: NSColor]

  /// The last style's look: making its fonts was a tenth of a patch batch.
  @MainActor private static var last: TextLook?

  @MainActor
  static func of(_ style: TranscriptStyle) -> TextLook {
    if let last, last.style == style { return last }
    let look = TextLook(style)
    last = look
    return look
  }

  private init(_ style: TranscriptStyle) {
    self.style = style
    line = TranscriptStyle.lines(style.body, style.lineHeights.body)
    let spacing = TranscriptStyle.lines(style.body, style.lineHeights.body)
    spacing.paragraphSpacing = style.blockSpacing
    self.spacing = spacing
    // A heading is bold by its token's weight, so a bold span in it keeps that weight.
    let faces =
      [(style.body, false), (style.code, false)]
      + TranscriptStyle.HeadingSize.allCases.map { (style.headings.font($0), true) }
    fonts = faces.flatMap { font, heading in
      [[], [.bold], [.italic], [.bold, .italic]].map { (traits: NSFontDescriptor.SymbolicTraits) in
        let traits = heading ? traits.subtracting(.bold) : traits
        return traits.isEmpty
          ? font
          : NSFont(descriptor: font.fontDescriptor.withSymbolicTraits(traits), size: font.pointSize)
            ?? font
      }
    }
    // One colour object per token, so equal runs merge (`Look.same`).
    colors = style.colors.merging([.text: style.text]) { own, _ in own }
    prompt = [
      .font: style.body, .foregroundColor: style.text, .transcriptDecor: Decor(.bubble, style),
    ]
    thought = [
      .font: style.thought.font, .foregroundColor: style.thought.color,
      .transcriptDecor: Decor(.thought, style),
    ]
    paragraphs = Paragraphs(style)
    rule = Look(
      font: style.body, color: style.text, extra: [.attachment: RuleAttachment(style.thought)])
  }

  func plain(_ token: StyleToken, code: Bool) -> Look {
    Look(font: code ? fonts[4] : fonts[0], color: colors[token] ?? style.text)
  }

  /// A span's own look; its `rgb` is left to the token, so every colour
  /// comes from the style.
  func look(_ span: Span, _ face: Face) -> Look {
    var look = Look(
      font: fonts[face.index * 4 + (span.bold ? 1 : 0) + (span.italic ? 2 : 0)],
      color: colors[span.token] ?? style.text)
    if span.strike { look.extra[.strikethroughStyle] = NSUnderlineStyle.single.rawValue }
    if span.underline { look.extra[.underlineStyle] = NSUnderlineStyle.single.rawValue }
    if let link = span.link.flatMap(URL.init(string:)) { look.extra[.link] = link }
    return look
  }
}

/// A piece's characters and looks as plain values, made into one attributed
/// string at the end: an attributed string per span was most of a 10 000-block
/// build's time.
struct Runs {
  private(set) var text = ""
  private(set) var length = 0
  private var looks: [(end: Int, look: TextLook.Look)] = []

  mutating func add(_ string: String, _ look: TextLook.Look) {
    guard !string.isEmpty else { return }
    text += string
    length += string.utf16.count
    if let last = looks.last, last.look.same(as: look) {
      looks[looks.count - 1].end = length
    } else {
      looks.append((length, look))
    }
  }

  func make() -> NSMutableAttributedString {
    let out = NSMutableAttributedString(string: text)
    var start = 0
    for (end, look) in looks {
      let range = NSRange(location: start, length: end - start)
      out.addAttribute(.font, value: look.font, range: range)
      out.addAttribute(.foregroundColor, value: look.color, range: range)
      if !look.extra.isEmpty { out.addAttributes(look.extra, range: range) }
      start = end
    }
    return out
  }
}

enum TranscriptText {
  /// Between two blocks and between the lines of one.
  static let separator = "\n"

  struct Built {
    let text: NSAttributedString
    let ranges: BlockRanges
    /// Each reply's `Piece.docStarts`, in the whole text's block ranges.
    let docStarts: [BlockID: [Int]]
  }

  /// One block's text; `docStarts` is where each of a reply's doc blocks
  /// starts in it, its leading separator included.
  typealias Piece = (text: NSMutableAttributedString, docStarts: [Int]?)

  /// A reply's text and where each of its doc blocks starts in it.
  typealias Doc = (text: NSMutableAttributedString, starts: [Int])

  /// The whole transcript. A block id seen twice keeps its first block, as
  /// `SessionStore` holds one block per id.
  @MainActor
  static func build(
    _ blocks: some Sequence<Block>, style: TranscriptStyle, cards: TranscriptCards
  ) -> Built {
    let look = TextLook.of(style)
    let out = NSMutableAttributedString()
    var ranges = BlockRanges()
    var docStarts: [BlockID: [Int]] = [:]
    for block in blocks where ranges.index(of: block.id) == nil {
      let piece = piece(block, look, cards: cards)
      if piece.text.length > 0, out.length > 0 {
        // Takes the previous block's attributes, so its last paragraph keeps its spacing.
        out.replaceCharacters(in: NSRange(location: out.length, length: 0), with: separator)
      }
      ranges.append(block.id, NSRange(location: out.length, length: piece.text.length))
      docStarts[block.id] = piece.docStarts
      out.append(piece.text)
    }
    return Built(text: out, ranges: ranges, docStarts: docStarts)
  }

  /// A card is one attachment character; a reply is its doc; any other block
  /// is its plain runs.
  @MainActor
  static func piece(_ block: Block, _ look: TextLook, cards: TranscriptCards) -> Piece {
    var out: NSMutableAttributedString
    var starts: [Int]?
    if TranscriptCards.isCard(block.kind) {
      out = NSMutableAttributedString(attachment: CardAttachment(block, cards: cards))
      out.addAttributes(
        look.plain(.text, code: false).attributes, range: NSRange(location: 0, length: out.length))
    } else if case .assistant(_, let doc, _) = block.kind {
      (out, starts) = self.doc(doc.blocks, look, continuing: false)
    } else if let decorated = decorated(block, look, cards: cards) {
      out = decorated
    } else {
      var plain = Runs()
      for run in runs(block.kind) where !run.text.isEmpty {
        if plain.length > 0 { plain.add(separator, look.plain(run.token, code: false)) }
        plain.add(run.text, look.plain(run.token, code: false))
      }
      out = plain.make()
    }
    respace(out, block: NSRange(location: 0, length: out.length), from: 0, look)
    return (out, starts)
  }

  /// Doc blocks as text, laid out as `run` lays them out: each after a
  /// separator when text comes before it (`continuing`: in the block too).
  static func doc(_ blocks: [DocBlock], _ look: TextLook, continuing: Bool) -> Doc {
    var out = Runs()
    var starts: [Int] = []
    var paragraphs: [(NSRange, Paragraph)] = []
    for block in blocks {
      starts.append(out.length)
      let code = if case .code = block { true } else { false }
      if continuing || out.length > 0, hasText(block) {
        out.add(separator, look.plain(.text, code: code))
      }
      styled(block, look, into: &out, &paragraphs)
    }
    let text = out.make()
    for (range, paragraph) in paragraphs {
      text.addAttribute(.transcriptParagraph, value: paragraph, range: range)
    }
    return (text, starts)
  }

  /// One doc block from its spans: the same characters as `run`, a list
  /// item's marker in front of its line (`TextLine.lead`). Each line with a
  /// paragraph style of its own notes it in `paragraphs`, over the line and the
  /// separator that ends it, so the paragraph's first character has it.
  static func styled(
    _ block: DocBlock, _ look: TextLook, into out: inout Runs,
    _ paragraphs: inout [(NSRange, Paragraph)]
  ) {
    func lines(_ lines: [TextLine], _ face: TextLook.Face, _ kind: TextKind?) {
      var open: (start: Int, paragraph: Paragraph?)?
      for (index, line) in lines.enumerated() {
        if index > 0 { out.add(separator, look.plain(.text, code: face == .code)) }
        if case (let start, let paragraph?)? = open {
          paragraphs.append((NSRange(start..<out.length), paragraph))
        }
        let own = kind.flatMap { look.paragraphs.of($0, line) }
        open = (out.length, own ?? (face == .code ? look.paragraphs.code : nil))
        out.add(line.lead, look.plain(.text, code: false))
        for span in line.spans { out.add(span.text, look.look(span, face)) }
      }
      if case (let start, let paragraph?)? = open, out.length > start {
        paragraphs.append((NSRange(start..<out.length), paragraph))
      }
    }
    switch block {
    case .text(let kind, let spans):
      let face: TextLook.Face =
        if case .heading(let level) = kind { .heading(.init(level: level)) } else { .body }
      lines(spans, face, kind)
    case .code(_, let spans): lines(spans.map { TextLine($0) }, .code, nil)
    case .table(let rows):
      let start = out.length
      out.add(run(block).text, look.plain(.text, code: false))
      if out.length > start {
        paragraphs.append((NSRange(start..<out.length), look.paragraphs.table(rows)))
      }
    case .rule: out.add(run(block).text, look.rule)
    }
  }

  static func hasText(_ block: DocBlock) -> Bool {
    func shows(_ spans: [Span]) -> Bool { spans.contains { !$0.text.isEmpty } }
    return switch block {
    case .text(_, let lines):
      lines.count > 1 || lines.first.map { !$0.lead.isEmpty || shows($0.spans) } == true
    case .code(_, let lines): lines.count > 1 || lines.first.map(shows) == true
    case .table, .rule: !run(block).text.isEmpty
    }
  }

  /// Only a block's last paragraph, with the separator that ends it, carries
  /// the block spacing; the others take the body's line spacing. `from` is where an edit began: paragraphs before it
  /// are as they were. Only characters whose spacing is wrong change, so a
  /// patch's storage edit stays inside what it changed.
  static func respace(
    _ text: NSMutableAttributedString, block: NSRange, from: Int, _ look: TextLook
  ) {
    guard block.length > 0 else { return }
    let decor = text.attribute(.transcriptDecor, at: block.location, effectiveRange: nil)
    if let decor = decor as? Decor { return decor.respace(text, block: block, from: from) }
    let string = text.mutableString
    let end = NSMaxRange(block) - 1
    let last = string.paragraphRange(for: NSRange(location: end, length: 0))
    let first = string.paragraphRange(
      for: NSRange(location: min(max(from, block.location), end), length: 0)
    ).location
    let spaced = NSRange(first..<NSMaxRange(last))
    var plain = NSRange()
    let own = text.attribute(
      .transcriptParagraph, at: first, longestEffectiveRange: &plain, in: spaced)
    if own != nil || plain != spaced { return restyle(text, spaced, last: last.location, look) }
    text.enumerateAttribute(.paragraphStyle, in: spaced) { value, range, _ in
      let inLast = NSIntersectionRange(range, last)
      let before = NSRange(range.location..<max(range.location, last.location))
      if before.length > 0, (value as? NSParagraphStyle) != look.line {
        text.addAttribute(.paragraphStyle, value: look.line, range: before)
      }
      if inLast.length > 0, (value as? NSParagraphStyle) != look.spacing {
        text.addAttribute(.paragraphStyle, value: look.spacing, range: inLast)
      }
    }
  }

  /// The text a block that is neither a card nor a reply shows. A turn's
  /// meta line shows on hover (DT§5.2), so it has no text.
  static func runs(_ kind: BlockKind) -> [(text: String, token: StyleToken)] {
    switch kind {
    case .notice(_, let text):
      return [(text, .text)]
    case .error(let text, _):
      return [(text, .error)]
    case .tool, .toolGroup, .approval, .question, .task, .assistant, .user, .thinking:
      // Cards: one attachment character (`TranscriptCards`); a reply: `doc`; a prompt and a
      // thought: `decorated`.
      return []
    case .compaction(_, _, _, let summary):
      return summary.map { [($0, .dim)] } ?? []
    case .checkpoint(let files):
      return [(files.joined(separator: separator), .dim)]
    case .turnMeta:
      return []
    }
  }

  /// A doc block's plain text, as `styled` draws it: a rule is its attachment's character.
  static func run(_ block: DocBlock) -> (text: String, code: Bool) {
    func joined(_ lines: [[Span]]) -> String {
      lines.map { $0.map(\.text).joined() }.joined(separator: separator)
    }
    switch block {
    case .text(_, let lines):
      return (
        lines.map { $0.lead + $0.spans.map(\.text).joined() }.joined(separator: separator), false
      )
    case .code(_, let lines): return (joined(lines), true)
    case .table(let rows):
      return (rows.map { $0.joined(separator: "\t") }.joined(separator: separator), false)
    case .rule: return (RuleAttachment.mark, false)
    }
  }
}
