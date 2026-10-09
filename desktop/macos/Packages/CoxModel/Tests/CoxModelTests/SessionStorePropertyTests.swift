// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// SessionStore's patch rules against generated patch lists (T63.1): the store must order
// blocks as a plain-array model of `cox_app::coalesce::apply` does, for any mix of upserts
// and removes, not only the lists SessionStoreTests spells out.

import CoxClient
import PropertyBased
import Testing

@testable import CoxModel

/// One timeline edit; `description` keeps a shrunk failure readable.
private enum Edit: Sendable, CustomStringConvertible {
  case upsert(BlockID, text: String, after: BlockID?)
  case remove(BlockID)

  var patch: TimelinePatch {
    switch self {
    case .upsert(let id, let text, let after):
      .upsert(block: Block(id: id, turn: 1, kind: .thinking(text: text)), after: after)
    case .remove(let id):
      .remove(id: id)
    }
  }

  var description: String {
    switch self {
    case .upsert(let id, let text, let after): "upsert(\(id), \(text), after: \(after ?? "nil"))"
    case .remove(let id): "remove(\(id))"
    }
  }
}

/// Five ids so edits collide; anchors b5 and b6 never exist, so those blocks append.
private func editLists() -> Generator<[Edit], some SendableSequenceType> {
  let id = Gen.int(in: 0...4).map { "b\($0)" }
  let anchor = Gen.int(in: -1...6).map { number -> BlockID? in number < 0 ? nil : "b\(number)" }
  let text = Gen.letter.string(of: 0...3)
  let edit = Gen<Edit>.oneOf(
    zip(id, text, anchor).map { Edit.upsert($0, text: $1, after: $2) },
    id.map { Edit.remove($0) })
  return edit.array(of: 0...40)
}

/// The ordering rule over a plain array: an existing id is replaced in place, `nil` inserts
/// first, a known anchor inserts after it, an unknown one appends.
private func reference(_ edits: [Edit]) -> [(id: BlockID, text: String)] {
  var rows: [(id: BlockID, text: String)] = []
  for edit in edits {
    switch edit {
    case .upsert(let id, let text, let after):
      if let found = rows.firstIndex(where: { $0.id == id }) {
        rows[found].text = text
        continue
      }
      let index =
        after.map { anchor in rows.firstIndex { $0.id == anchor }.map { $0 + 1 } ?? rows.count }
        ?? 0
      rows.insert((id, text), at: index)
    case .remove(let id):
      rows.removeAll { $0.id == id }
    }
  }
  return rows
}

@MainActor
private func emptyStore() -> SessionStore {
  SessionStore(session: FixtureSession(fixture: Fixture(batches: [], snapshot: [])))
}

@MainActor
@Suite struct SessionStoreProperties {
  @Test func ordersBlocksAsTheReferenceModel() async {
    await propertyCheck(count: 300, input: editLists()) { edits in
      let store = emptyStore()
      store.apply(edits.map(\.patch))
      let expected = reference(edits)
      #expect(Array(store.blocks.keys) == expected.map(\.id))
      #expect(store.blocks.values.map(\.kind) == expected.map { .thinking(text: $0.text) })
    }
  }

  @Test func repeatingTheLastEditChangesNothing() async {
    await propertyCheck(input: editLists(), editLists().filter { !$0.isEmpty }) { edits, tail in
      let once = emptyStore()
      once.apply((edits + tail).map(\.patch))
      let twice = emptyStore()
      twice.apply((edits + tail + [tail[tail.count - 1]]).map(\.patch))
      #expect(once.blocks == twice.blocks)
    }
  }

  @Test func splittingABatchDoesNotChangeTheResult() async {
    await propertyCheck(input: editLists(), Gen.int(in: 0...40)) { edits, cut in
      let whole = emptyStore()
      whole.apply(edits.map(\.patch))
      let split = emptyStore()
      let boundary = min(cut, edits.count)
      split.apply(edits[..<boundary].map(\.patch))
      split.apply(edits[boundary...].map(\.patch))
      #expect(whole.blocks == split.blocks)
    }
  }

  @Test func resetKeepsTheFirstPositionAndTheLastValue() async {
    await propertyCheck(input: editLists()) { edits in
      let blocks = edits.compactMap { edit -> Block? in
        guard case .upsert(let id, let text, _) = edit else { return nil }
        return Block(id: id, turn: 1, kind: .thinking(text: text))
      }
      let store = emptyStore()
      store.apply([.reset(blocks: blocks)])
      var firstSeen: [BlockID] = []
      for block in blocks where !firstSeen.contains(block.id) { firstSeen.append(block.id) }
      #expect(Array(store.blocks.keys) == firstSeen)
      for id in firstSeen {
        #expect(store.blocks[id] == blocks.last { $0.id == id })
      }
    }
  }

  @Test func lastLinesKeepsAtMostFiveLinesOfTheEnd() async {
    let text = Gen.int(in: 0...2).map { ["a", "\n", "\r\n"][$0] }.array(of: 0...60)
      .map { $0.joined() }
    await propertyCheck(count: 500, input: text) { text in
      let tail = lastLines(text)
      let body = tail.utf8.last == UInt8(ascii: "\n") ? tail.utf8.dropLast() : tail.utf8[...]
      #expect(text.hasSuffix(tail))
      #expect(body.filter { $0 == UInt8(ascii: "\n") }.count < tailLines)
    }
  }
}
