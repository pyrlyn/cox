// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Review's line comments (T37.28.4, DT§5.4): the draft a line-number click adds to and "Send to
// agent" posts as one turn, queued behind a running turn like a composer prompt unless
// `[desktop.review] send = "now"` (A108). Here, not in CoxUI, because the anchor is read from the
// open file's diff and the draft outlives the open file (it lives on `SessionStore`); the prompt's
// words are cox-app's (`SessionClient.reviewMessage`), so every surface sends the same.

import CoxClient
import Foundation

public struct ReviewDraft: Equatable, Sendable {
  /// In the order they were written.
  public var comments: [LineComment] = []
  /// The line a click picked, its text still being typed; `nil` while none is.
  public var editing: LineComment?

  public init(comments: [LineComment] = [], editing: LineComment? = nil) {
    (self.comments, self.editing) = (comments, editing)
  }

  /// Starts a comment on line `line` of hunk `hunk` of `review`'s open diff, anchored at its
  /// number on disk, or at its number before the change for a removed line. An index outside
  /// the diff picks nothing.
  public mutating func pick(_ review: ReviewState, hunk: Int, line: Int) {
    guard let path = review.selection, let hunks = review.diff?.hunks,
      hunks.indices.contains(hunk), hunks[hunk].lines.indices.contains(line)
    else { return }
    let picked = hunks[hunk].lines[line]
    guard let number = picked.new ?? picked.old else { return }
    editing = LineComment(path: path, line: number, removed: picked.new == nil, text: "")
  }

  /// Adds the picked line with `text`, trimmed; blank text drops it instead.
  public mutating func save(_ text: String) {
    defer { editing = nil }
    let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
    guard var comment = editing, !trimmed.isEmpty else { return }
    comment.text = trimmed
    comments.append(comment)
  }

  public mutating func remove(at index: Int) {
    guard comments.indices.contains(index) else { return }
    comments.remove(at: index)
  }
}

/// `[desktop.review] send`, spelled as Rust stores it (A108): cox-app's `SendWhen`, which also
/// decides a review's queueing (T58.4.19).
public typealias ReviewSend = SendWhen

extension SettingsStore {
  /// `[desktop.review] send` from the loaded view; queue before the first load or when the key
  /// holds something else.
  public var reviewSend: ReviewSend {
    view.flatMap { SectionRows($0.settings, "desktop.review").decode("send") } ?? .queue
  }
}

extension SessionStore {
  /// Posts the draft as one turn in cox-app's words — queued behind the running turn while one
  /// runs, unless `when` is `.now` — and empties it once the core took it; sends nothing while no
  /// comment has text.
  public func sendReview(_ when: ReviewSend = .queue) async throws {
    guard let text = session.reviewMessage(reviewDraft.comments) else { return }
    let draft = session.draftIntent(
      text, shell: false, attachments: 0, running: isTurnRunning, when: when)
    _ = try await send(
      draft.queued
        ? .queue(text: text, attachments: [], confirmThink: false)
        : .send(text: text, attachments: [], confirmThink: false))
    reviewDraft = ReviewDraft()
  }
}
