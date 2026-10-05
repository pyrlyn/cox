// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Timeline patches into the text (T37.43, DT§4.5): each patch edits only its
// own block's range, so a streamed reply or thought appends to the storage
// instead of rebuilding it. The pieces come from `TranscriptText`, the
// bookkeeping from `BlockRanges`; this file owns only the splice between
// them, which keeps one separator between blocks that have text.

import AppKit
import CoxClient

extension TranscriptTextView {
  /// Applies one batch in one storage edit. `current` gives a block as the
  /// batch left it (the `SessionStore` that applied it first), for the copy
  /// map and a card's view; the text itself follows the patches.
  public func apply(_ patches: [TimelinePatch], current: (BlockID) -> Block?) {
    guard let storage = textStorage else { return }
    let look = TextLook.of(style)
    let cards = hostedCards
    storage.beginEditing()
    defer { storage.endEditing() }
    for patch in patches {
      switch patch {
      case .reset(let all):
        load(all)
      case .upsert(let block, let after):
        upsert(block, after: after, look, cards)
      case .appendText(let id, let text):
        append(text, to: id, current: current, look)
      case .docTail(let id, let from, let tail):
        replaceTail(of: id, from: Int(from), with: tail, current: current, look)
      case .remove(let id):
        remove(id, look)
      case .usage, .status, .pluginSlot:
        continue
      }
    }
  }

  private func remove(_ id: BlockID, _ look: TextLook) {
    guard let index = blockRanges.index(of: id) else { return }
    let whole = NSRange(location: 0, length: blockRanges.ranges[index].length)
    splice(index, whole, with: .init(), look)
    blockRanges.remove(at: index)
    (blocks[id], docStarts[id]) = (nil, nil)
  }

  /// A thought grows at its end; a card shows its block's new tail.
  private func append(
    _ text: String, to id: BlockID, current: (BlockID) -> Block?, _ look: TextLook
  ) {
    guard let index = blockRanges.index(of: id) else { return }
    defer { blocks[id] = current(id) ?? blocks[id] }
    if case .thinking(let old, let durationMs)? = blocks[id]?.kind, var grown = blocks[id] {
      grown.kind = .thinking(text: old + text, durationMs: durationMs)
      thoughtGrew(index, by: text, to: current(id) ?? grown, look)
    } else if let block = current(id) {
      card(at: index)?.update(block)
    }
  }

  /// A reply's doc blocks from `from` on are replaced; those before it stay.
  private func replaceTail(
    of id: BlockID, from: Int, with tail: [DocBlock], current: (BlockID) -> Block?,
    _ look: TextLook
  ) {
    guard let index = blockRanges.index(of: id), let starts = docStarts[id], from <= starts.count
    else { return }
    defer { blocks[id] = current(id) ?? blocks[id] }
    let length = blockRanges.ranges[index].length
    let cut = from < starts.count ? starts[from] : length
    let doc = TranscriptText.doc(tail, look, continuing: cut > 0)
    splice(index, NSRange(location: cut, length: length - cut), with: doc.text, look)
    docStarts[id] = starts.prefix(from) + doc.starts.map { $0 + cut }
  }

  /// A known block is replaced in place, a card keeping its view; a new one
  /// goes after `after` (`nil`: first; unknown: last), as `SessionStore` does.
  private func upsert(_ block: Block, after: BlockID?, _ look: TextLook, _ cards: TranscriptCards) {
    let known = blockRanges.index(of: block.id)
    blocks[block.id] = block
    if let known, TranscriptCards.isCard(block.kind), let card = card(at: known) {
      card.update(block)
      return
    }
    let piece = TranscriptText.piece(block, look, cards: cards)
    docStarts[block.id] = piece.docStarts
    guard let known else {
      let index = after.map { blockRanges.index(of: $0).map { $0 + 1 } ?? blockRanges.count } ?? 0
      blockRanges.insert(block.id, at: index)
      splice(index, NSRange(location: 0, length: 0), with: piece.text, spaced: true, look)
      return
    }
    let whole = NSRange(location: 0, length: blockRanges.ranges[known].length)
    splice(known, whole, with: piece.text, spaced: true, look)
  }

  private func card(at index: Int) -> CardAttachment? {
    let range = blockRanges.ranges[index]
    guard range.length == 1 else { return nil }
    return textStorage?.attribute(.attachment, at: range.location, effectiveRange: nil)
      as? CardAttachment
  }

  /// Replaces `local`, a range inside block `index`'s text, with `text`
  /// (`spaced`: a whole piece, its block spacing set). A block gaining its
  /// first text gains the separator after it, or before it when no text
  /// follows; one losing its last text drops the same one. So a separator
  /// always inherits the block before it, as `build` gives it.
  func splice(
    _ index: Int, _ local: NSRange, with text: NSAttributedString, spaced: Bool = false,
    _ look: TextLook
  ) {
    guard let storage = textStorage else { return }
    let old = blockRanges.ranges[index]
    let length = old.length - local.length + text.length
    let gains = old.length == 0 && length > 0
    let loses = old.length > 0 && length == 0
    let before = blockRanges.text(before: index) != nil
    let after = (gains || loses) && blockRanges.hasText(after: index)
    let separated = (gains || loses) && (before || after)
    var range = NSRange(location: old.location, length: length)
    var edit = NSRange(location: old.location + local.location, length: local.length)
    if gains, after, before {
      (range.location, edit.location) = (old.location + 1, old.location + 1)
    } else if gains, before {
      range.location += 1
    } else if loses, separated {
      edit.length += 1
      if !after { edit.location -= 1 }
      if before { range.location = old.location - 1 }
    }
    storage.replaceCharacters(in: edit, with: text)
    if gains, separated {
      // The character before it lends the separator its attributes.
      let location = after ? NSMaxRange(range) : old.location
      let point = NSRange(location: location, length: 0)
      storage.replaceCharacters(in: point, with: TranscriptText.separator)
    }
    let delta = text.length - edit.length + (gains && separated ? 1 : 0)
    blockRanges.update(index, to: range, delta: delta)
    if !spaced { TranscriptText.respace(storage, block: range, from: edit.location, look) }
  }
}
