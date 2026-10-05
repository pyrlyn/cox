// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Whole-block selection from the gutter (T37.42.2, DT§5.2): a click left of
// the text selects the block beside it, and a ⇧-click selects every block
// from that anchor to the clicked one. The gutter is the text container's
// leading inset, which the host sizes (DT§5: 36 pt). Its own file because it
// adds only the gutter click; the clamp lives in `Selection.swift`, copy in
// `MarkdownCopy.swift`.

import AppKit

extension TranscriptTextView {
  /// Installs the gutter click; `make(style:)` calls it once.
  func addGutterClick() {
    addGestureRecognizer(GutterClick(target: self, action: #selector(gutterClicked(_:))))
  }

  @objc private func gutterClicked(_ click: GutterClick) {
    guard let block = click.block else { return }
    selectBlocks(to: block, extending: click.extending)
  }

  /// Selects blocks `anchor…clicked` whole, or only the clicked one without
  /// `extending`. With `crossBlockSelection` off it stays in the anchor
  /// block, as a drag stays in the block it started in.
  func selectBlocks(to clicked: Int, extending: Bool) {
    let anchor = extending ? selectionAnchor() ?? clicked : clicked
    let (low, high) =
      crossBlockSelection ? (min(anchor, clicked), max(anchor, clicked)) : (anchor, anchor)
    let start = blockRanges.ranges[low].location
    gutterAnchor = blockRanges.ids[anchor]
    setSelectedRange(NSRange(location: start, length: NSMaxRange(blockRanges.ranges[high]) - start))
  }

  /// The last gutter click's block while the selection still covers it,
  /// else the block the selection starts in.
  private func selectionAnchor() -> Int? {
    let selected = selectedRange()
    if let index = gutterAnchor.flatMap(blockRanges.index(of:)) {
      let range = blockRanges.ranges[index]
      if range.length > 0, NSIntersectionRange(range, selected).length > 0 { return index }
    }
    return blockRanges.index(at: selected.location)
  }

  /// The block whose line sits at `point`'s height, `point` in view
  /// coordinates; TextKit 2 only, as everywhere in this package.
  func blockIndex(besides point: NSPoint) -> Int? {
    guard let layout = textLayoutManager, let content = layout.textContentManager,
      let fragment = layout.textLayoutFragment(
        for: CGPoint(x: 0, y: point.y - textContainerOrigin.y))
    else { return nil }
    let start = content.documentRange.location
    return blockRanges.index(at: content.offset(from: start, to: fragment.rangeInElement.location))
  }
}

/// A click in the gutter, recognized on mouse down and held back from the
/// text view. A recognizer rather than a `mouseDown` override: overriding it
/// sends `NSTextView` down its blocking tracking loop instead of TextKit 2's
/// own click handling. Any other click fails at once and reaches the view.
final class GutterClick: NSGestureRecognizer {
  private(set) var block: Int?
  private(set) var extending = false

  override init(target: Any?, action: Selector?) {
    super.init(target: target, action: action)
    delaysPrimaryMouseButtonEvents = true
  }

  required init?(coder: NSCoder) { nil }

  override func mouseDown(with event: NSEvent) {
    guard let view = view as? TranscriptTextView else {
      state = .failed
      return
    }
    let point = view.convert(event.locationInWindow, from: nil)
    block = point.x < view.textContainerOrigin.x ? view.blockIndex(besides: point) : nil
    extending = event.modifierFlags.contains(.shift)
    if block == nil { view.gutterAnchor = nil }
    state = block == nil ? .failed : .ended
  }
}
