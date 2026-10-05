// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// T37.22.5: Review's pane from CoxModel's `ReviewState` and the store's draft — a turn's files
// under `Turn n`, the open diff's hunks, and each comment anchored at `path:line`, the open one
// included.

import CoxClient
import CoxModel
import CoxUI
import Testing

@testable import CoxTranscript

private let notesFile = ChangedFile(
  path: "notes.md", change: .edited, added: 1, removed: 1, call: "c1", turn: 1)
private let newFile = ChangedFile(
  path: "new.rs", change: .created, added: 1, removed: 0, call: "c2", turn: 2)

private let changes = Changes(
  files: [notesFile, newFile],
  checkpoints: [Checkpoint(turn: 1, label: "Turn 1 · before notes.md", time: "")],
  turns: [TurnFiles(turn: 1, files: [notesFile]), TurnFiles(turn: 2, files: [newFile])])

private let notes = DiffModel(
  path: "notes.md",
  hunks: [
    DiffHunk(
      header: "@@ -1 +1 @@",
      lines: [
        DiffLine(kind: .del, old: 1, new: nil, spans: []),
        DiffLine(kind: .add, old: nil, new: 1, spans: []),
      ])
  ])

@Test func reviewsFilesDiffAndCommentsFillThePane() {
  let review = ReviewState(changes, selection: "notes.md", diff: notes)
  var draft = ReviewDraft()
  draft.pick(review, hunk: 0, line: 1)
  draft.save("Keep the heading")
  draft.pick(review, hunk: 0, line: 0)

  let pane = ReviewPane.State(review, draft: draft)
  #expect(pane.turns.map(\.title) == ["Turn 1", "Turn 2"])
  #expect(pane.turns.map { $0.files.map(\.path) } == [["notes.md"], ["new.rs"]])
  #expect(pane.turns[1].files[0].change == .created)
  #expect(pane.timeline.checkpoints.map(\.id) == ["1"])
  #expect(pane.selection == "notes.md")
  #expect(pane.hunks.map(\.header) == ["@@ -1 +1 @@"])
  #expect(pane.hunks[0].lines.map(\.number) == ["1", "1"])
  #expect(pane.comments == [ReviewPane.Comment(anchor: "notes.md:1", text: "Keep the heading")])
  #expect(pane.editing == "notes.md:1")
}

@Test func noOpenFileLeavesTheDiffAndDraftEmpty() {
  let pane = ReviewPane.State(ReviewState(changes), draft: ReviewDraft())
  #expect(pane.hunks.isEmpty)
  #expect(pane.selection == nil)
  #expect(pane.comments.isEmpty && pane.editing == nil)
}
