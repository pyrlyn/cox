// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The composer editor's own text and selection (T37.24.9, DS§6.4 row `Composer`), and the
// selection as UTF-16 offsets: the store finds the `@` or `/` token at the caret from plain
// offsets and puts the caret after a picked row with them. Separate from `Composer.swift` so the
// organism keeps to its layout.

import SwiftUI

/// What the editor holds. `NSTextView` reports a keystroke's selection before its text, and a
/// binding read from `Composer.State` would hand the editor the old text until the next state
/// arrives — the editor then puts the caret back at the start — so the editor edits this copy
/// and the composer reports each change and takes each state the store sends back.
struct ComposerDraft: Equatable {
  var text: String
  var selection: TextSelection?

  /// The store's text and selection, as a keystroke, a pick, a recalled prompt or a send left them.
  init(_ state: Composer.State) {
    text = state.text
    selection = TextSelection(utf16: state.selectedRange, in: state.text)
  }

  /// What changed since `state`: the text first, since the selection's offsets are into it.
  func intents(after state: Composer.State) -> [Composer.Intent] {
    let edit: [Composer.Intent] = text == state.text ? [] : [.edit(text)]
    return edit + (selection?.utf16Range(in: text).map { [.select($0)] } ?? [])
  }
}

extension TextSelection {
  /// A selection in `text` from UTF-16 offsets; no range, or one past the end, is the caret at
  /// the end — the editor would put a `nil` selection at the start.
  init(utf16 range: Range<Int>?, in text: String) {
    let utf16 = text.utf16
    guard let range, range.lowerBound >= 0, range.upperBound <= utf16.count else {
      self.init(insertionPoint: text.endIndex)
      return
    }
    let lower = utf16.index(utf16.startIndex, offsetBy: range.lowerBound)
    self.init(range: lower..<utf16.index(lower, offsetBy: range.count))
  }

  /// UTF-16 offsets of the last range into `text`; `nil` when it runs past `text`.
  func utf16Range(in text: String) -> Range<Int>? {
    let range: Range<String.Index>
    switch indices {
    case .selection(let one): range = one
    case .multiSelection(let set):
      guard let last = set.ranges.last else { return nil }
      range = last
    @unknown default: return nil
    }
    let utf16 = text.utf16
    guard let lower = String.Index(range.lowerBound, within: utf16),
      let upper = String.Index(range.upperBound, within: utf16)
    else { return nil }
    let start = utf16.distance(from: utf16.startIndex, to: lower)
    return start..<(start + utf16.distance(from: lower, to: upper))
  }
}
