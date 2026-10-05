// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The one-block clamp (T37.42, A67): with `crossBlockSelection` off, every
// selection the text view sets is cut to one block — the block a drag started
// in, whichever way it runs. Its own file because it hooks AppKit's selection
// path and nothing else; copy lives in `MarkdownCopy.swift`.

import AppKit

extension TranscriptTextView {
  override public func setSelectedRanges(
    _ ranges: [NSValue], affinity: NSSelectionAffinity, stillSelecting: Bool
  ) {
    guard !crossBlockSelection, let first = ranges.first?.rangeValue,
      let anchor = anchor(for: first, stillSelecting: stillSelecting)
    else {
      dragAnchor = nil
      super.setSelectedRanges(ranges, affinity: affinity, stillSelecting: stillSelecting)
      return
    }
    // A drag sends `stillSelecting` until the mouse goes up; the block it
    // started in holds for all of it.
    dragAnchor = stillSelecting ? anchor : nil
    let bound = blockRanges.ranges[anchor]
    let clamped = ranges.map { NSValue(range: Self.clamp($0.rangeValue, to: bound)) }
    super.setSelectedRanges(clamped, affinity: affinity, stillSelecting: stillSelecting)
  }

  /// The block a new selection is held to: a running drag's own block, else
  /// the block of the range's start. A settled change (a key extending the
  /// selection, select all) that still touches the current selection's block
  /// stays there, so ⇧→ never leaves it; one elsewhere (find) moves.
  private func anchor(for range: NSRange, stillSelecting: Bool) -> Int? {
    if let dragAnchor, blockRanges.ranges.indices.contains(dragAnchor) { return dragAnchor }
    guard !stillSelecting, let current = blockRanges.index(at: selectedRange().location) else {
      return blockRanges.index(at: range.location)
    }
    let bound = blockRanges.ranges[current]
    let touches = range.location <= NSMaxRange(bound) && NSMaxRange(range) >= bound.location
    return touches ? current : blockRanges.index(at: range.location)
  }

  /// `range` cut to `bound`; a range wholly outside it becomes a caret at the
  /// edge it lies beyond.
  static func clamp(_ range: NSRange, to bound: NSRange) -> NSRange {
    let low = max(range.location, bound.location)
    let high = min(NSMaxRange(range), NSMaxRange(bound))
    if high >= low { return NSRange(location: low, length: high - low) }
    let edge = range.location < bound.location ? bound.location : NSMaxRange(bound)
    return NSRange(location: edge, length: 0)
  }
}
