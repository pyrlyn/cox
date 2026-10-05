// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `TranscriptTextView` (T37.40, DT§5.2): one TextKit 2 `NSTextView` over the
// whole transcript, so AppKit's own drag selection runs across blocks — the
// engine spike T37.37 chose over per-block views (research.md §9.5.13). Its own
// file because the view is the one AppKit type the app hosts; the text and the
// block ranges it shows come from `TranscriptText` and `BlockRanges`.

import AppKit
import CoxClient

public final class TranscriptTextView: NSTextView {
  public private(set) var style = TranscriptStyle.system
  /// Where each block's text sits; kept in step with the text storage.
  public internal(set) var blockRanges = BlockRanges()
  /// The shown blocks by id, for copy (`MarkdownCopy.swift`).
  public internal(set) var blocks: [BlockID: Block] = [:]
  /// Where each reply's doc blocks start in its text (`TranscriptPatches.swift`).
  var docStarts: [BlockID: [Int]] = [:]
  /// Whether a drag may run across blocks (A67). The app passes
  /// `[desktop.transcript] cross_block_selection`; `false` clamps a selection
  /// to the block it started in (`Selection.swift`).
  public var crossBlockSelection = true
  /// The block a clamped drag started in, while the drag runs.
  var dragAnchor: Int?
  /// The block a ⇧-click in the gutter extends from (`BlockSelection.swift`).
  var gutterAnchor: BlockID?
  /// Where "Copy as Markdown" writes; tests pass a private one.
  var markdownPasteboard = NSPasteboard.general
  /// The views card blocks show (T37.41); set before `load`.
  public var cards = TranscriptCards.summary
  /// The thoughts the reader opened; any other shows folded (`TranscriptDecor.swift`).
  public internal(set) var openThoughts: Set<BlockID> = []
  /// The prompt the pointer is over, its actions' view and its gutter's (`PromptHover.swift`).
  var promptHover: (id: BlockID, view: NSView, gutter: NSView?)?
  /// The open menu of a prompt's gutter, if any (`PromptHover.swift`).
  public internal(set) var promptMenu: NSPopover?

  /// A read-only, selectable transcript on TextKit 2. `NSTextView()` would
  /// also be TextKit 2, but this names it: reading `layoutManager` falls back
  /// to TextKit 1 for good, so nothing in this package ever does.
  public static func make(style: TranscriptStyle = .system) -> TranscriptTextView {
    let view = TranscriptTextView(usingTextLayoutManager: true)
    view.style = style
    view.isEditable = false
    view.isSelectable = true
    view.isRichText = true
    view.textContainerInset = style.inset
    view.isVerticallyResizable = true
    view.isHorizontallyResizable = false
    view.autoresizingMask = [.width]
    view.textContainer?.widthTracksTextView = true
    view.addGutterClick()
    view.trackPrompts()
    view.textLayoutManager?.delegate = DecorLayout.shared
    return view
  }

  override public func setFrameSize(_ newSize: NSSize) {
    super.setFrameSize(newSize)
    fitColumn()
  }

  /// Keeps the lines in the style's reading column as the view's width changes.
  private func fitColumn() {
    let inset = style.inset(width: frame.width, padding: textContainer?.lineFragmentPadding ?? 0)
    if inset != textContainerInset { textContainerInset = inset }
  }

  /// A vertically scrolling host, sized to `frame`, with this view as its document.
  public func inScrollView(frame: NSRect) -> NSScrollView {
    let scroll = NSScrollView(frame: frame)
    scroll.hasVerticalScroller = true
    scroll.autoresizingMask = [.width, .height]
    let content = scroll.contentSize
    self.frame = NSRect(origin: .zero, size: content)
    minSize = NSSize(width: 0, height: content.height)
    maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: .greatestFiniteMagnitude)
    scroll.documentView = self
    return scroll
  }

  /// Replaces the whole text with `blocks`, in order.
  public func load(_ blocks: some Sequence<Block>) {
    let blocks = Array(blocks)
    let built = TranscriptText.build(blocks, style: style, cards: hostedCards)
    blockRanges = built.ranges
    docStarts = built.docStarts
    self.blocks = Dictionary(blocks.map { ($0.id, $0) }) { first, _ in first }
    dragAnchor = nil
    textStorage?.setAttributedString(built.text)
  }

  /// Draws the same text with `style` (T37.23.6), as when the text size
  /// changes. The text is built again and only its attributes are copied over,
  /// so the characters, the block ranges, the selection and each thought's
  /// open or folded state stay; each card keeps its attachment, and with it its
  /// view and that view's own state.
  public func restyle(_ style: TranscriptStyle) {
    guard style != self.style, let storage = textStorage else { return }
    self.style = style
    fitColumn()
    let shown = blockRanges.ids.compactMap { blocks[$0] }
    let built = TranscriptText.build(shown, style: style, cards: hostedCards)
    // Patches keep the text what a load gives (T37.43); should that ever slip, load it anew.
    guard built.text.string == storage.string, built.ranges == blockRanges else {
      return load(shown)
    }
    let selection = selectedRanges
    storage.beginEditing()
    let whole = NSRange(location: 0, length: built.text.length)
    built.text.enumerateAttributes(in: whole) { attributes, range, _ in
      var attributes = attributes
      if attributes[.attachment] is CardAttachment {
        let card = storage.attribute(.attachment, at: range.location, effectiveRange: nil)
        attributes[.attachment] = (card as? CardAttachment) ?? attributes[.attachment]
      }
      storage.setAttributes(attributes, range: range)
    }
    storage.endEditing()
    docStarts = built.docStarts
    selectedRanges = selection
  }

  public func range(of id: BlockID) -> NSRange? { blockRanges.range(of: id) }

  /// The block whose text holds `location` (see `BlockRanges.index(at:)`).
  public func blockID(at location: Int) -> BlockID? { blockRanges.blockID(at: location) }
}

extension TranscriptStyle {
  /// The text container's inset in a view `width` wide: `inset`, widened on both sides so the
  /// lines — the container less its line fragment `padding` each side — are `readingWidth` wide.
  func inset(width: CGFloat, padding: CGFloat) -> NSSize {
    guard let readingWidth else { return inset }
    let centred = (width - readingWidth) / 2 - padding
    return NSSize(width: max(inset.width, centred), height: inset.height)
  }
}
