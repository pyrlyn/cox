// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Review for one session (DT§5.4, T37.28, T37.22.5): `SessionStore.review` and the draft on the
// store copied into CoxUI's `ReviewPane`, whose intents go back to the store: a line-number click
// picks the line, "Send to agent" posts the draft as `[desktop.review] send` says (A108), the
// timeline rewinds, and a hunk's "Revert hunk" (T51.21) reverts it; a stale revert's refusal is
// the core's Notice in the transcript. Separate from `SessionInspector` because Review replaces
// the transcript column.

import CoxClient
import CoxModel
import CoxUI
import SwiftUI

/// The changed files by turn, the rewind timeline and the open file's diff with its comments.
public struct SessionReview: View {
  let store: SessionStore
  let path: String?
  let reviewSend: ReviewSend
  let refused: @MainActor (String) -> Void
  @State private var review = ReviewState()
  /// The file the pane opened since it showed; `path` until it opens another.
  @State private var opened: String?

  public init(
    store: SessionStore, path: String?, reviewSend: ReviewSend,
    refused: @escaping @MainActor (String) -> Void
  ) {
    (self.store, self.path, self.reviewSend, self.refused) = (store, path, reviewSend, refused)
  }

  public var body: some View {
    ReviewPane(state: ReviewPane.State(review, draft: store.reviewDraft), send: handle)
      .task(id: Reload(path: opened ?? path, blocks: store.blocks.count)) {
        do {
          review = try await store.review(path: opened ?? path)
        } catch {
          refused(String(describing: error))
        }
      }
      .onChange(of: path) { opened = nil }
  }

  /// When the diff is read again: another file, or the session changed one.
  private struct Reload: Equatable {
    let path: String?
    let blocks: Int
  }

  private func handle(_ intent: ReviewPane.Intent) {
    switch intent {
    case .open(let path): opened = path
    case .comment(let hunk, let line): store.reviewDraft.pick(review, hunk: hunk, line: line)
    case .save(let text): store.reviewDraft.save(text)
    case .remove(let index): store.reviewDraft.remove(at: index)
    case .sendComments: perform { try await store.sendReview(reviewSend) }
    case .revertHunk(let hunk):
      let shown = review
      perform { try await store.revert(hunk: hunk, in: shown) }
    case .timeline(.rewind(let checkpoint, let code, let conversation)):
      perform {
        try await store.rewind(checkpoint: checkpoint, code: code, conversation: conversation)
      }
    }
  }

  private func perform(_ work: @escaping @MainActor () async throws -> Void) {
    Task {
      do {
        try await work()
      } catch {
        refused(String(describing: error))
      }
    }
  }
}

extension ReviewPane.State {
  /// A comment's anchor is `path:line`, the number `ReviewDraft` picked.
  init(_ review: ReviewState, draft: ReviewDraft) {
    let anchor = { (comment: LineComment) in "\(comment.path):\(comment.line)" }
    self.init(
      turns: review.turns.map {
        ReviewPane.Turn(title: "Turn \($0.turn)", files: $0.files.map(ChangedFileRow.File.init))
      },
      timeline: RewindTimeline.State(
        checkpoints: review.checkpoints.map(CheckpointRow.Checkpoint.init)),
      selection: review.selection,
      hunks: review.diff?.hunks.map(ToolCard.Hunk.init) ?? [],
      comments: draft.comments.map { ReviewPane.Comment(anchor: anchor($0), text: $0.text) },
      editing: draft.editing.map(anchor),
      revertsHunks: review.diff?.digest != nil)
  }
}
