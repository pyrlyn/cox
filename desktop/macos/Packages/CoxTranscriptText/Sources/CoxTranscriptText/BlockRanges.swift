// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Where each timeline block sits in the transcript's text (T37.40): block id →
// range and location → block id. Separate from the view so selection clamping
// (T37.42) and incremental edits (T37.43) work on one value type that a test
// can build without AppKit laying anything out.

import CoxClient
import Foundation

/// The text range of every block, in transcript order.
///
/// A block with text owns its characters; the one-character separator before
/// the next block's text belongs to no block. A block without text (a turn's
/// meta line, a thinking block before its first delta) takes a zero-length
/// range at the end of the text before it and adds no line.
public struct BlockRanges: Equatable, Sendable {
  public private(set) var ids: [BlockID] = []
  public private(set) var ranges: [NSRange] = []
  private var indexByID: [BlockID: Int] = [:]

  public init() {}

  public var count: Int { ids.count }

  /// Records the next block in transcript order; the caller skips an id it
  /// already recorded.
  mutating func append(_ id: BlockID, _ range: NSRange) {
    indexByID[id] = ids.count
    ids.append(id)
    ranges.append(range)
  }

  /// Inserts a block without text at `index`, at the end of the text before it.
  mutating func insert(_ id: BlockID, at index: Int) {
    ids.insert(id, at: index)
    ranges.insert(NSRange(location: textEnd(before: index), length: 0), at: index)
    for later in index..<ids.count { indexByID[ids[later]] = later }
  }

  /// Drops a block that no longer has text.
  mutating func remove(at index: Int) {
    indexByID[ids.remove(at: index)] = nil
    ranges.remove(at: index)
    for later in index..<ids.count { indexByID[ids[later]] = later }
  }

  /// Block `index` now holds `range` after an edit that changed the text's
  /// length by `delta`: later blocks move by `delta`, and those without text
  /// right after it to the end of the text at or before it.
  mutating func update(_ index: Int, to range: NSRange, delta: Int) {
    ranges[index] = range
    let end = range.length > 0 ? NSMaxRange(range) : textEnd(before: index)
    var textSeen = false
    for later in ranges.indices.dropFirst(index + 1) {
      textSeen = textSeen || ranges[later].length > 0
      ranges[later].location = textSeen ? ranges[later].location + delta : end
    }
  }

  /// The nearest block before `index` that has text.
  func text(before index: Int) -> Int? {
    ranges[..<index].lastIndex { $0.length > 0 }
  }

  func hasText(after index: Int) -> Bool {
    ranges[(index + 1)...].contains { $0.length > 0 }
  }

  /// Where a block without text at `index` sits.
  func textEnd(before index: Int) -> Int { text(before: index).map { NSMaxRange(ranges[$0]) } ?? 0 }

  public func index(of id: BlockID) -> Int? { indexByID[id] }

  public func range(of id: BlockID) -> NSRange? { index(of: id).map { ranges[$0] } }

  /// The block whose text holds `location`, counting a caret right after a
  /// block's last character as inside it; `nil` past the end of the text.
  /// A block without text never holds a location.
  public func index(at location: Int) -> Int? {
    guard location >= 0 else { return nil }
    // The last block starting at or before `location`: binary search, since
    // starts never decrease.
    var low = 0
    var high = ranges.count
    while low < high {
      let mid = (low + high) / 2
      if ranges[mid].location <= location { low = mid + 1 } else { high = mid }
    }
    var index = low - 1
    while index >= 0, ranges[index].length == 0 { index -= 1 }
    guard index >= 0, location <= NSMaxRange(ranges[index]) else { return nil }
    return index
  }

  public func blockID(at location: Int) -> BlockID? { index(at: location).map { ids[$0] } }
}
