// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Changes tab's state from cox-app's `Changes` (T37.29.1): the files, the checkpoints and
// the worktree's facts fill the tab, read through SessionStore from the fixture session. What
// the facts say (`detached`, `Base` only with both) is `cox_app::changes`'s test (T58.4.20).

import CoxClient
import Foundation
import Testing

@testable import CoxModel

private let changes = Changes(
  files: [
    ChangedFile(path: "src/retry.rs", change: .edited, added: 18, removed: 4, call: "c1", turn: 1),
    ChangedFile(
      path: "tests/backoff.rs", change: .created, added: 12, removed: 0, call: "c2", turn: 1),
    ChangedFile(path: "src/old.rs", change: .deleted, added: 0, removed: 30, call: "c3", turn: 1),
  ],
  checkpoints: [
    Checkpoint(
      turn: 1, label: "Turn 1 · before retry.rs and 1 more", time: "2026-09-28T14:02:07.123Z")
  ],
  worktree: Linked(
    path: "/w/_worktrees/cox-t1", branch: "t1", base: "main", commit: "4273daa",
    bytes: 412_000_000),
  worktreeFacts: [
    Fact(label: "Branch", value: "t1"), Fact(label: "Base", value: "main @ 4273daa"),
  ])

@MainActor
@Test func theStoreFillsTheTabFromTheSession() async throws {
  let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []), changes: changes)
  let tab = try await SessionStore(session: session).changesTab()
  #expect(tab == ChangesTabState(changes))
  #expect((tab.files.count, tab.checkpoints.count, tab.worktree.count) == (3, 1, 3))
}

@Test func theMappingFormatsTheRowsTheTabShows() {
  let tab = ChangesTabState(changes, locale: Locale(identifier: "en_GB"), timeZone: .gmt)
  #expect(
    tab.files == [
      .init(path: "src/retry.rs", change: .edited, added: 18, removed: 4),
      .init(path: "tests/backoff.rs", change: .created, added: 12, removed: 0),
      .init(path: "src/old.rs", change: .deleted, added: 0, removed: 30),
    ])
  #expect(
    tab.checkpoints == [.init(id: "1", label: "Turn 1 · before retry.rs and 1 more", time: "14:02")]
  )
  #expect(
    tab.worktree == [
      .init(label: "Branch", values: ["t1"]),
      .init(label: "Base", values: ["main @ 4273daa"]),
      .init(label: "Size", values: ["412 MB"]),
    ])
}

@Test func outsideALinkedWorktreeTheTabHasNoFacts() {
  let tab = ChangesTabState(Changes(files: changes.files, checkpoints: changes.checkpoints))
  #expect(tab.worktree.isEmpty)
  #expect(ChangesTabState(Changes()) == ChangesTabState())
}
