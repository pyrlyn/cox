// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Candidate B: our own TextKit 2 view. One NSTextView holds the whole transcript,
// so AppKit's own drag selection runs across blocks; tool cards are view-backed
// attachments (one attachment character each). Copy writes Markdown in block
// order; `crossBlockSelection = false` clamps every selection to the block the
// drag started in.

import AppKit

extension NSAttributedString.Key {
  static let coxBlock = NSAttributedString.Key("coxBlock")
}

public final class TranscriptTextView: NSTextView {
  public var crossBlockSelection = true
  public private(set) var blocks: [Block] = []
  /// Content range of each block in the text storage (block separators excluded).
  public private(set) var blockRanges: [NSRange] = []
  private var dragAnchorBlock: Int?

  /// A scroll view with a TextKit 2 transcript view as its document.
  public static func make(blocks: [Block], size: NSSize) -> (NSScrollView, TranscriptTextView) {
    let scroll = NSScrollView(frame: NSRect(origin: .zero, size: size))
    scroll.hasVerticalScroller = true
    scroll.autoresizingMask = [.width, .height]
    let view = TranscriptTextView(usingTextLayoutManager: true)
    let content = scroll.contentSize
    view.frame = NSRect(origin: .zero, size: content)
    view.minSize = NSSize(width: 0, height: content.height)
    view.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: .greatestFiniteMagnitude)
    view.isVerticallyResizable = true
    view.isHorizontallyResizable = false
    view.autoresizingMask = [.width]
    view.textContainer?.widthTracksTextView = true
    view.textContainerInset = NSSize(width: 16, height: 16)
    view.isEditable = false
    view.isSelectable = true
    view.isRichText = true
    view.load(blocks)
    scroll.documentView = view
    return (scroll, view)
  }

  public func load(_ blocks: [Block]) {
    let (storage, ranges) = Self.render(blocks)
    self.blocks = blocks
    self.blockRanges = ranges
    textStorage?.setAttributedString(storage)
  }

  // MARK: Rendering

  static let bodyFont = NSFont.systemFont(ofSize: 13.5)
  static let monoFont = NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)

  static func render(_ blocks: [Block]) -> (NSAttributedString, [NSRange]) {
    let out = NSMutableAttributedString()
    var ranges: [NSRange] = []
    ranges.reserveCapacity(blocks.count)
    let spacing = NSMutableParagraphStyle()
    spacing.paragraphSpacing = 10
    for block in blocks {
      if out.length > 0 {
        out.append(NSAttributedString(string: "\n", attributes: [.font: bodyFont]))
      }
      let piece = render(block)
      piece.addAttribute(
        .coxBlock, value: block.id, range: NSRange(location: 0, length: piece.length))
      piece.addAttribute(
        .paragraphStyle, value: spacing, range: NSRange(location: 0, length: piece.length))
      ranges.append(NSRange(location: out.length, length: piece.length))
      out.append(piece)
    }
    return (out, ranges)
  }

  static func render(_ block: Block) -> NSMutableAttributedString {
    switch block.kind {
    case .prose:
      return inlineMarkdown(block.body)
    case .code:
      return NSMutableAttributedString(
        string: block.body,
        attributes: [.font: monoFont, .backgroundColor: NSColor.quaternarySystemFill])
    case .diff:
      let out = NSMutableAttributedString()
      for (index, line) in block.body.split(separator: "\n", omittingEmptySubsequences: false)
        .enumerated()
      {
        let color: NSColor =
          line.hasPrefix("+") ? .systemGreen : line.hasPrefix("-") ? .systemRed : .labelColor
        let text = (index == 0 ? "" : "\n") + line
        out.append(
          NSAttributedString(string: text, attributes: [.font: monoFont, .foregroundColor: color]))
      }
      return out
    case .tool:
      let attachment = ToolCardAttachment(block: block)
      return NSMutableAttributedString(attachment: attachment)
    }
  }

  /// Inline Markdown (bold, code) to fonts. In the app this comes from Rust (`StyledDoc`, T37.7).
  static func inlineMarkdown(_ markdown: String) -> NSMutableAttributedString {
    let options = AttributedString.MarkdownParsingOptions(
      interpretedSyntax: .inlineOnlyPreservingWhitespace)
    let parsed =
      (try? AttributedString(markdown: markdown, options: options))
      ?? AttributedString(markdown)
    let out = NSMutableAttributedString()
    for run in parsed.runs {
      let text = String(parsed[run.range].characters)
      let intent = run.inlinePresentationIntent ?? []
      let font: NSFont =
        intent.contains(.code)
        ? monoFont
        : intent.contains(.stronglyEmphasized) ? .boldSystemFont(ofSize: 13.5) : bodyFont
      out.append(NSAttributedString(string: text, attributes: [.font: font]))
    }
    return out
  }

  // MARK: Blocks

  /// The block whose content holds (or precedes) `location`.
  public func blockIndex(at location: Int) -> Int {
    var low = 0
    var high = blockRanges.count - 1
    while low < high {
      let mid = (low + high + 1) / 2
      if blockRanges[mid].location <= location { low = mid } else { high = mid - 1 }
    }
    return low
  }

  // MARK: Clamping (cross_block_selection = false)

  public override func setSelectedRanges(
    _ ranges: [NSValue], affinity: NSSelectionAffinity, stillSelecting: Bool
  ) {
    guard !crossBlockSelection, !blockRanges.isEmpty, let first = ranges.first?.rangeValue else {
      super.setSelectedRanges(ranges, affinity: affinity, stillSelecting: stillSelecting)
      return
    }
    let anchor =
      dragAnchorBlock
      ?? blockIndex(at: stillSelecting ? first.location : selectedRange().location)
    dragAnchorBlock = stillSelecting ? anchor : nil
    let bound = blockRanges[anchor]
    let clamped = ranges.map { value -> NSValue in
      let range = value.rangeValue
      let low = max(range.location, bound.location)
      let high = min(NSMaxRange(range), NSMaxRange(bound))
      if high >= low { return NSValue(range: NSRange(location: low, length: high - low)) }
      let edge = range.location < bound.location ? bound.location : NSMaxRange(bound)
      return NSValue(range: NSRange(location: edge, length: 0))
    }
    super.setSelectedRanges(clamped, affinity: affinity, stillSelecting: stillSelecting)
  }

  // MARK: Copy as Markdown

  /// The selection as Markdown, blocks in transcript order. A whole block gives its Markdown
  /// source; part of a block gives the selected text (fenced for code and diffs).
  public func markdownForSelection() -> String {
    let text = (string as NSString)
    var parts: [String] = []
    for value in selectedRanges {
      let range = value.rangeValue
      guard range.length > 0 else { continue }
      var index = blockIndex(at: range.location)
      while index < blocks.count, blockRanges[index].location < NSMaxRange(range) {
        let full = blockRanges[index]
        let part = NSIntersectionRange(range, full)
        if part.length > 0 {
          let block = blocks[index]
          if NSEqualRanges(part, full) || block.kind == .tool {
            parts.append(block.markdown)
          } else {
            let selected = text.substring(with: part)
            switch block.kind {
            case .prose, .tool: parts.append(selected)
            case .code: parts.append("```swift\n\(selected)\n```")
            case .diff: parts.append("```diff\n\(selected)\n```")
            }
          }
        }
        index += 1
      }
    }
    return parts.joined(separator: "\n\n")
  }

  public override var writablePasteboardTypes: [NSPasteboard.PasteboardType] { [.string] }

  public override func writeSelection(
    to pboard: NSPasteboard, types: [NSPasteboard.PasteboardType]
  ) -> Bool {
    pboard.clearContents()
    return pboard.setString(markdownForSelection(), forType: .string)
  }
}

// MARK: - Tool card attachment

final class ToolCardAttachment: NSTextAttachment {
  let block: Block

  init(block: Block) {
    self.block = block
    super.init(data: nil, ofType: nil)
  }

  required init?(coder: NSCoder) { nil }

  override func viewProvider(
    for parentView: NSView?, location: any NSTextLocation, textContainer: NSTextContainer?
  ) -> NSTextAttachmentViewProvider? {
    let provider = ToolCardViewProvider(
      textAttachment: self, parentView: parentView,
      textLayoutManager: textContainer?.textLayoutManager, location: location)
    provider.tracksTextAttachmentViewBounds = true
    return provider
  }

  override func attachmentBounds(
    for attributes: [NSAttributedString.Key: Any], location: any NSTextLocation,
    textContainer: NSTextContainer?, proposedLineFragment: CGRect, position: CGPoint
  ) -> CGRect {
    CGRect(x: 0, y: 0, width: max(proposedLineFragment.width - 8, 120), height: 44)
  }
}

final class ToolCardViewProvider: NSTextAttachmentViewProvider {
  override func loadView() {
    guard let card = textAttachment as? ToolCardAttachment else { return }
    view = ToolCardView(block: card.block)
  }
}

final class ToolCardView: NSView {
  private let title = NSTextField(labelWithString: "")
  private let detail = NSTextField(labelWithString: "")

  init(block: Block) {
    super.init(frame: NSRect(x: 0, y: 0, width: 400, height: 44))
    wantsLayer = true
    layer?.cornerRadius = 8
    layer?.borderWidth = 1
    layer?.borderColor = NSColor.separatorColor.cgColor
    layer?.backgroundColor = NSColor.controlBackgroundColor.cgColor
    title.stringValue = block.tool ?? "tool"
    title.font = .boldSystemFont(ofSize: 12)
    detail.stringValue = block.body
    detail.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
    detail.lineBreakMode = .byTruncatingTail
    addSubview(title)
    addSubview(detail)
  }

  required init?(coder: NSCoder) { nil }

  override func layout() {
    super.layout()
    title.frame = NSRect(x: 10, y: 24, width: bounds.width - 20, height: 16)
    detail.frame = NSRect(x: 10, y: 5, width: bounds.width - 20, height: 16)
  }
}
