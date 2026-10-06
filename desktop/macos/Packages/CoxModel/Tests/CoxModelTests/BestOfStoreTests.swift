// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// T52.11: the compare view's store reads the columns, lists what a keep prunes, and never
// discards a worktree with changes on the first yes.

import CoxClient
import Testing

@testable import CoxModel

private func column(
  _ index: UInt32, _ candidate: Candidate, _ state: CandidateState
) -> CandidateView {
  CandidateView(
    index: index, candidate: candidate, state: state, session: "s\(index)",
    worktree: "/w/best-1-\(index + 1)", branch: "best-1-\(index + 1)",
    files: [FileStat(path: "Form.tsx", added: 10 + index, removed: 2)], costUsd: 0.5)
}

private let three = [
  column(0, .cox(model: nil), .done),
  column(1, .agent(name: "claude"), .done),
  column(2, .cox(model: "gpt-5"), .running),
]

/// A suite so `--filter BestOf` finds these.
@MainActor
struct BestOfStoreTests {
  @Test func bestOfStoreReadsTheColumnsAgainOnRefresh() async {
    let client = FixtureBestOf(three)
    let store = BestOfStore(id: "1", client: client)
    await store.refresh()
    #expect(store.columns.map(\.label) == ["cox", "claude", "cox · gpt-5"])
    #expect(store.columns[0].added == 10)
    var moved = three
    moved[2].state = .done
    client.set(moved)
    await store.refresh()
    #expect(store.columns[2].state == .done)
  }

  @Test func bestOfStoreListsWhatAKeepPrunes() async {
    let store = BestOfStore(id: "1", client: FixtureBestOf(three))
    await store.refresh()
    #expect(store.prunes(keeping: 1).map(\.index) == [0, 2])
    await store.keep(1)
    #expect(store.columns.map(\.state) == [.pruned, .kept, .pruned])
    #expect(store.pending == nil)
    #expect(store.prunes(keeping: 1).isEmpty)
  }

  @Test func bestOfStoreAsksAgainBeforeDiscardingChanges() async {
    let store = BestOfStore(id: "1", client: FixtureBestOf(three, dirty: ["/w/best-1-3"]))
    await store.refresh()
    await store.keep(0)
    #expect(store.pending == BestOfStore.Pending(keep: 0, dirty: ["/w/best-1-3"]))
    #expect(store.columns[2].state == .running, "a worktree with changes stays after one yes")
    store.keepChanges()
    #expect(store.pending == nil)
    await store.keep(0)
    await store.discard()
    #expect(store.pending == nil)
    #expect(store.columns.map(\.state) == [.kept, .pruned, .pruned])
  }

  @Test func bestOfFailedCandidateWithoutChangesOffersNoActions() {
    var failed = column(0, .cox(model: nil), .failed(why: "provider error: provider auth failed"))
    failed.files = []
    #expect(!failed.canReview)
    #expect(!failed.canKeep(anotherKept: false))
    failed.files = [FileStat(path: "Form.tsx", added: 1, removed: 0)]
    #expect(failed.canReview, "a failed turn that wrote files can still be looked at")
    #expect(!failed.canKeep(anotherKept: false), "but never kept")
  }

  @Test func bestOfKeepsTheOtherActionRules() {
    let done = column(0, .cox(model: nil), .done)
    #expect(done.canReview)
    #expect(done.canKeep(anotherKept: false))
    #expect(!done.canKeep(anotherKept: true))
    #expect(!column(1, .cox(model: nil), .pruned).canReview)
    #expect(column(2, .cox(model: nil), .running).canReview)
  }

  @Test func bestOfCostNeverPrintsNegativeZero() {
    #expect(usd(-0.0) == "$0.00")
    #expect(usd(-0.004) == "$0.00")
    #expect(usd(0) == "$0.00")
    #expect(usd(0.376) == "$0.38")
    #expect(usd(-1.5) == "$-1.50")
  }
}
