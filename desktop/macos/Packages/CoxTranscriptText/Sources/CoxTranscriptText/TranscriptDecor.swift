// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A prompt's bubble and a thought's fold in the transcript text (T37.23.4,
// DT§5.2): both stay text, so a drag can start partway through a prompt and run
// on into the reply, which a card attachment (selected whole) cannot give. A
// `Decor` attribute marks their paragraphs; a layout fragment draws the bubble
// or the rule behind them. A thought's open or folded state is the view's own
// (`openThoughts`): folding edits only the text after its header, so no other
// block's range moves. Its own file because it is the one place that draws
// behind the text.

import AppKit
import CoxClient

extension NSAttributedString.Key {
  /// The `Decor` of a prompt's or a thought's characters.
  static let transcriptDecor = NSAttributedString.Key("cox.transcript.decor")
  /// A decorated paragraph's `Decor.Edge`, as its raw value.
  static let transcriptEdge = NSAttributedString.Key("cox.transcript.edge")
  /// A prompt's turn, as an `Int`, drawn in the gutter beside its first line (T37.47).
  static let transcriptTurn = NSAttributedString.Key("cox.transcript.turn")
}

/// How a prompt's or a thought's paragraphs are set and what is drawn behind
/// them. One per style and kind (`TextLook`).
final class Decor: NSObject {
  enum Kind { case bubble, thought }

  /// Whether a paragraph is its block's first, last, both or neither, and whether it is a
  /// prompt's row of tiles under its text.
  struct Edge: OptionSet {
    let rawValue: Int
    static let first = Edge(rawValue: 1)
    static let last = Edge(rawValue: 2)
    static let tiles = Edge(rawValue: 4)
  }

  let kind: Kind
  let bubble: TranscriptStyle.Bubble
  private let thought: TranscriptStyle.Thought
  private let gutter: TranscriptStyle.Gutter?
  /// One paragraph style per `Edge` raw value.
  private var styles: [NSParagraphStyle] = []

  init(_ kind: Kind, _ style: TranscriptStyle) {
    (self.kind, bubble, thought, gutter) = (kind, style.bubble, style.thought, style.gutter)
    super.init()
    styles = (0..<8).map { raw in
      let edge = Edge(rawValue: raw)
      let paragraph =
        kind == .bubble
        ? TranscriptStyle.lines(style.body, style.lineHeights.body)
        : TranscriptStyle.lines(thought.font, style.lineHeights.thought)
      let spacing = edge.contains(.last) ? style.blockSpacing : 0
      switch kind {
      case .bubble:
        let side = bubble.padding.width
        (paragraph.firstLineHeadIndent, paragraph.headIndent, paragraph.tailIndent) = (
          side, side, -side
        )
        paragraph.paragraphSpacingBefore =
          edge.contains(.first) ? bubble.padding.height : edge.contains(.tiles) ? bubble.gap : 0
        paragraph.paragraphSpacing = spacing + (edge.contains(.last) ? bubble.padding.height : 0)
      case .thought:
        // The first paragraph is the fold header, at the margin; the reasoning sits past the rule.
        let indent = edge.contains(.first) ? 0 : thought.indent
        (paragraph.firstLineHeadIndent, paragraph.headIndent) = (indent, indent)
        paragraph.paragraphSpacing = spacing
      }
      return paragraph
    }
  }

  /// `TranscriptText.respace` for a decorated block: each paragraph from
  /// `from`'s on takes its edge's style, and only one whose style is wrong changes.
  private func opensWithTile(_ text: NSAttributedString, _ paragraph: NSRange) -> Bool {
    text.attribute(.attachment, at: paragraph.location, effectiveRange: nil) != nil
  }

  func respace(_ text: NSMutableAttributedString, block: NSRange, from: Int) {
    let string = text.mutableString
    let end = NSMaxRange(block)
    let start = min(max(from, block.location), end - 1)
    var location = string.paragraphRange(for: NSRange(location: start, length: 0)).location
    while location < end {
      let paragraph = string.paragraphRange(for: NSRange(location: location, length: 0))
      var edge: Edge = paragraph.location <= block.location ? .first : []
      if NSMaxRange(paragraph) >= end { edge.insert(.last) }
      // A prompt's own characters are never attachments: one opening a later paragraph is a tile.
      if kind == .bubble, !edge.contains(.first), opensWithTile(text, paragraph) {
        edge.insert(.tiles)
      }
      let style = styles[edge.rawValue]
      text.enumerateAttribute(.paragraphStyle, in: paragraph) { value, range, _ in
        guard (value as? NSParagraphStyle) !== style else { return }
        text.addAttributes([.paragraphStyle: style, .transcriptEdge: edge.rawValue], range: range)
      }
      location = NSMaxRange(paragraph)
    }
  }

  /// What a paragraph at `edge` draws behind its lines, in its fragment's
  /// coordinates: its slice of the bubble, or the rule beside the reasoning.
  func area(_ fragment: NSTextLayoutFragment, _ edge: Edge) -> CGRect? {
    guard let first = fragment.textLineFragments.first?.typographicBounds,
      let last = fragment.textLineFragments.last?.typographicBounds
    else { return nil }
    let container = fragment.textLayoutManager?.textContainer
    // The fragment's frame starts at its text, past the paragraph's indent: the
    // bubble and the rule start at the text column's edge, as unindented text does.
    let frame = fragment.layoutFragmentFrame
    let margin = (container?.lineFragmentPadding ?? 0) - frame.minX
    let height = frame.height
    switch kind {
    case .bubble:
      let width = (container?.size.width ?? frame.width) - 2 * (container?.lineFragmentPadding ?? 0)
      let top = edge.contains(.first) ? first.minY - bubble.padding.height : 0
      let bottom = edge.contains(.last) ? last.maxY + bubble.padding.height : height
      return CGRect(x: margin, y: top, width: width, height: bottom - top)
    case .thought:
      guard !edge.contains(.first) else { return nil }
      let bottom = edge.contains(.last) ? last.maxY : height
      return CGRect(x: margin, y: 0, width: thought.ruleWidth, height: bottom)
    }
  }

  /// Where a prompt's first paragraph draws its turn number, in its fragment's coordinates: the
  /// gutter's box left of the bubble, as tall as the first line. `nil` for any other paragraph or
  /// without a gutter.
  func gutterArea(_ fragment: NSTextLayoutFragment, _ edge: Edge) -> CGRect? {
    guard kind == .bubble, edge.contains(.first), let gutter, let face = area(fragment, edge),
      let line = fragment.textLineFragments.first?.typographicBounds
    else { return nil }
    return CGRect(
      x: face.minX - gutter.offset, y: line.minY, width: gutter.width, height: line.height)
  }

  /// Draws `turn` right-aligned in `box`, centred on its line.
  func drawTurn(_ turn: Int, in box: CGRect, context: CGContext) {
    guard let gutter else { return }
    let paragraph = NSMutableParagraphStyle()
    paragraph.alignment = .right
    let number = NSAttributedString(
      string: String(turn),
      attributes: [.font: gutter.font, .foregroundColor: gutter.color, .paragraphStyle: paragraph])
    let height = number.size().height
    let line = CGRect(x: box.minX, y: box.midY - height / 2, width: box.width, height: height)
    NSGraphicsContext.saveGraphicsState()
    defer { NSGraphicsContext.restoreGraphicsState() }
    // The text view is flipped, and so is the context TextKit hands a fragment.
    NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: true)
    number.draw(with: line, options: [.usesLineFragmentOrigin])
  }

  /// Draws a paragraph's slice `rect` of the decoration; `whole` is the bubble it is a slice of.
  func draw(_ rect: CGRect, whole: CGRect, _ edge: Edge, in context: CGContext) {
    context.saveGState()
    defer { context.restoreGState() }
    switch kind {
    case .bubble:
      // The whole bubble is drawn and clipped to the slice, so its round ends, sweep and
      // shadows run on across the paragraphs as one shape. A line height leaves slices at
      // fractional offsets, where two antialiased clips would both tint the pixel row they
      // share: snapped to device pixels, each row is one slice's.
      let rect = Self.snapped(rect, in: context)
      let radius = min(bubble.radius, whole.width / 2, whole.height / 2)
      let shape = CGPath(
        roundedRect: whole, cornerWidth: radius, cornerHeight: radius, transform: nil)
      dropShadows(shape, rect, edge, in: context)
      context.clip(to: rect)
      context.addPath(shape)
      context.clip()
      for color in [bubble.face, bubble.fill] {
        context.setFillColor(color.cgColor)
        context.fill(whole)
      }
      let stops = bubble.sweep
      let sweep = CGGradient(
        colorsSpace: nil, colors: stops.map(\.color.cgColor) as CFArray,
        locations: stops.map(\.location))
      if !stops.isEmpty, let sweep {
        context.drawLinearGradient(
          sweep, start: whole.origin, end: CGPoint(x: whole.maxX, y: whole.maxY), options: [])
      }
      for layer in bubble.shadows where layer.inset {
        // The highlight: the bubble less itself shrunk by the spread and moved by the offset.
        let inner = whole.insetBy(dx: layer.spread, dy: layer.spread)
          .offsetBy(dx: layer.offset.width, dy: layer.offset.height)
        let corner = max(0, radius - layer.spread)
        context.addPath(shape)
        context.addPath(
          CGPath(roundedRect: inner, cornerWidth: corner, cornerHeight: corner, transform: nil))
        context.setFillColor(layer.color.cgColor)
        context.fillPath(using: .evenOdd)
      }
    case .thought:
      context.setFillColor(thought.rule.cgColor)
      context.fill(rect)
    }
  }

  /// `rect` with each edge on the nearest device pixel, so two slices sharing an edge meet
  /// there exactly.
  private static func snapped(_ rect: CGRect, in context: CGContext) -> CGRect {
    let device = context.convertToDeviceSpace(rect)
    let (minX, minY) = (device.minX.rounded(), device.minY.rounded())
    let snapped = CGRect(
      x: minX, y: minY, width: device.maxX.rounded() - minX, height: device.maxY.rounded() - minY)
    return context.convertToUserSpace(snapped)
  }

  /// The bubble's drop shadows around the slice `rect`: past its sides, and past the bubble's
  /// own ends — a cut edge is the next slice's to draw. Never under the face, which lets the
  /// wallpaper show through.
  private func dropShadows(_ shape: CGPath, _ rect: CGRect, _ edge: Edge, in context: CGContext) {
    let reach = bubble.reach
    var outer = rect.insetBy(dx: -reach, dy: 0)
    if edge.contains(.first) {
      (outer.origin.y, outer.size.height) = (outer.minY - reach, outer.height + reach)
    }
    if edge.contains(.last) { outer.size.height += reach }
    // A shadow's offset and blur are in device space: the transform scales them and, in a
    // flipped view, turns down into its negative. CSS blur is twice Core Graphics', as it is
    // twice SwiftUI's shadow radius.
    let ctm = context.ctm
    // A drop shadow's spread is not drawn: the shape casting it is clipped away, so a larger one
    // would show; no elevation token spreads one (DS§3.4).
    for layer in bubble.shadows where !layer.inset {
      context.saveGState()
      context.clip(to: outer)
      context.addRect(outer)
      context.addPath(shape)
      context.clip(using: .evenOdd)
      context.setShadow(
        offset: CGSize(width: layer.offset.width * ctm.a, height: layer.offset.height * ctm.d),
        blur: layer.blur / 2 * abs(ctm.a), color: layer.color.cgColor)
      context.addPath(shape)
      context.setFillColor(NSColor.black.cgColor)
      context.fillPath()
      context.restoreGState()
    }
  }
}

/// A decorated paragraph's layout: the bubble or the rule under its text.
final class DecorFragment: NSTextLayoutFragment {
  var decoration: (decor: Decor, edge: Decor.Edge)? {
    guard let text = (textElement as? NSTextParagraph)?.attributedString, text.length > 0,
      let decor = text.attribute(.transcriptDecor, at: 0, effectiveRange: nil) as? Decor
    else { return nil }
    let raw = text.attribute(.transcriptEdge, at: 0, effectiveRange: nil) as? Int
    return (decor, Decor.Edge(rawValue: raw ?? 0))
  }

  /// The prompt's turn, on its first paragraph only.
  var turn: Int? {
    guard let text = (textElement as? NSTextParagraph)?.attributedString, text.length > 0 else {
      return nil
    }
    return text.attribute(.transcriptTurn, at: 0, effectiveRange: nil) as? Int
  }

  override var renderingSurfaceBounds: CGRect {
    let bounds = super.renderingSurfaceBounds
    guard let (decor, edge) = decoration, let area = decor.area(self, edge) else { return bounds }
    let reach = decor.kind == .bubble ? decor.bubble.reach : 0
    let surface = bounds.union(area.insetBy(dx: -reach, dy: -reach))
    return decor.gutterArea(self, edge).map { surface.union($0) } ?? surface
  }

  override func draw(at point: CGPoint, in context: CGContext) {
    if let (decor, edge) = decoration, let area = decor.area(self, edge) {
      let whole = decor.kind == .bubble ? bubble(area, decor, edge) : area
      decor.draw(
        area.offsetBy(dx: point.x, dy: point.y), whole: whole.offsetBy(dx: point.x, dy: point.y),
        edge, in: context)
      if let turn, let box = decor.gutterArea(self, edge) {
        decor.drawTurn(turn, in: box.offsetBy(dx: point.x, dy: point.y), context: context)
      }
    }
    super.draw(at: point, in: context)
  }

  /// The whole bubble `slice` is part of, in this fragment's coordinates: the slices of its
  /// block's paragraphs laid out so far, from the first to the last.
  func bubble(_ slice: CGRect, _ decor: Decor, _ edge: Decor.Edge) -> CGRect {
    guard let manager = textLayoutManager else { return slice }
    var whole = slice
    let origin = layoutFragmentFrame.origin
    func join(_ options: NSTextLayoutFragment.EnumerationOptions, until end: Decor.Edge) {
      manager.enumerateTextLayoutFragments(from: rangeInElement.location, options: options) {
        if $0.rangeInElement.isEqual(to: self.rangeInElement) { return true }
        guard let other = $0 as? DecorFragment, let (kind, edge) = other.decoration,
          kind === decor, let area = kind.area(other, edge)
        else { return false }
        let frame = other.layoutFragmentFrame
        whole = whole.union(area.offsetBy(dx: frame.minX - origin.x, dy: frame.minY - origin.y))
        return !edge.contains(end)
      }
    }
    if !edge.contains(.first) { join(.reverse, until: .first) }
    if !edge.contains(.last) { join([], until: .last) }
    return whole
  }
}

/// Gives a decorated paragraph its `DecorFragment` and a quote line its
/// `QuoteFragment` (`TranscriptStructure.swift`); holds no state, so every
/// transcript shares one.
final class DecorLayout: NSObject, NSTextLayoutManagerDelegate {
  @MainActor static let shared = DecorLayout()

  func textLayoutManager(
    _ textLayoutManager: NSTextLayoutManager, textLayoutFragmentFor location: any NSTextLocation,
    in textElement: NSTextElement
  ) -> NSTextLayoutFragment {
    let text = (textElement as? NSTextParagraph)?.attributedString
    let decorated =
      text.map {
        $0.length > 0 && $0.attribute(.transcriptDecor, at: 0, effectiveRange: nil) != nil
      }
      ?? false
    let range = textElement.elementRange
    if decorated { return DecorFragment(textElement: textElement, range: range) }
    return QuoteFragment.quoted(text)
      ? QuoteFragment(textElement: textElement, range: range)
      : NSTextLayoutFragment(textElement: textElement, range: range)
  }
}

extension TranscriptText {
  /// A prompt, with its attachments' tiles on a line under it; a thought, its
  /// fold header and, open, its reasoning under it (none while it has no text).
  /// `nil` for any other block.
  @MainActor
  static func decorated(
    _ block: Block, _ look: TextLook, cards: TranscriptCards
  ) -> NSMutableAttributedString? {
    let out = NSMutableAttributedString()
    switch block.kind {
    case .user(let text, let attachments):
      out.append(NSAttributedString(string: text, attributes: look.prompt))
      if !attachments.isEmpty, out.length > 0 {
        out.append(NSAttributedString(string: separator, attributes: look.prompt))
      }
      for name in attachments {
        let tile = CardAttachment(block, cards: cards, role: .thumbnail(name))
        var attributes = look.prompt
        (attributes[.attachment], attributes[.kern]) = (tile, look.style.bubble.gap)
        out.append(NSAttributedString(string: "\u{FFFC}", attributes: attributes))
      }
      out.addAttribute(
        .transcriptTurn, value: Int(block.turn), range: NSRange(location: 0, length: out.length))
    case .thinking(let text, _):
      guard !text.isEmpty else { break }
      let header = CardAttachment(block, cards: cards, role: .header)
      out.append(NSAttributedString(attachment: header))
      if cards.isOpen(block.id) { out.append(NSAttributedString(string: separator + text)) }
      out.addAttributes(look.thought, range: NSRange(location: 0, length: out.length))
    default:
      return nil
    }
    return out
  }
}

extension TranscriptTextView {
  /// Opens or folds a thought. Only the text after its header changes, so
  /// every other block keeps its range and the header keeps its view.
  public func setThought(_ id: BlockID, open: Bool) {
    guard openThoughts.contains(id) != open else { return }
    if open { openThoughts.insert(id) } else { openThoughts.remove(id) }
    guard let index = blockRanges.index(of: id), let block = blocks[id],
      case .thinking(let text, _) = block.kind
    else { return }
    let range = blockRanges.ranges[index]
    guard range.length > 0 else { return }
    let look = TextLook.of(style)
    let tail =
      open
      ? NSAttributedString(string: TranscriptText.separator + text, attributes: look.thought)
      : NSAttributedString()
    splice(index, NSRange(location: 1, length: range.length - 1), with: tail, look)
    let header = textStorage?.attribute(.attachment, at: range.location, effectiveRange: nil)
    (header as? CardAttachment)?.update(block)
  }

  /// A thought gained `text`: its first text brings the header; an open one
  /// shows the text at its end, a folded one only keeps it.
  func thoughtGrew(_ index: Int, by text: String, to block: Block, _ look: TextLook) {
    blocks[block.id] = block
    let length = blockRanges.ranges[index].length
    if length == 0 {
      let piece = TranscriptText.piece(block, look, cards: hostedCards)
      splice(index, NSRange(location: 0, length: 0), with: piece.text, spaced: true, look)
    } else if openThoughts.contains(block.id) {
      let grown = NSAttributedString(string: text, attributes: look.thought)
      splice(index, NSRange(location: length, length: 0), with: grown, look)
    }
  }
}
