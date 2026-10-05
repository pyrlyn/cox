// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The rewind timeline's intent (T37.28.1, DT§5.4): a checkpoint of `ChangesTabState`, whose id
// is the turn, and the scope the person picked become `Intent.rewind`; a changed file's Revert
// (T37.28.3) becomes `Intent.revertFile`, and a Review hunk's (T51.21) `Intent.revertHunk`. Here,
// beside the mapping that wrote the id, so the app never parses it; the core does the rewind
// itself. A prompt's Edit and resend (T37.23.18, A102) rewinds too, the conversation only, to
// before its turn.

import CoxClient

extension SessionStore {
  /// Rewinds code, the conversation or both to before the checkpoint's turn. An id that is no
  /// turn — none that `ChangesTabState` writes — sends nothing.
  public func rewind(checkpoint id: String, code: Bool, conversation: Bool) async throws {
    guard let turn = UInt32(id) else { return }
    try await rewind(toTurn: turn, code: code, conversation: conversation)
  }

  /// Rewinds to before `turn`, the first turn the core undoes: the timeline's checkpoints and a
  /// prompt's rewind menu (Figma frame 14) alike.
  public func rewind(toTurn turn: UInt32, code: Bool, conversation: Bool) async throws {
    _ = try await send(.rewind(toTurn: turn, code: code, conversation: conversation))
  }

  /// Restores one changed file, as the Changes tab lists it, to before the session changed it:
  /// its pre-image from before turn 1. The core checkpoints it first, so `redo` undoes it.
  public func revert(path: String) async throws {
    _ = try await send(.revertFile(path: path, toTurn: 1))
  }

  /// Reverts the hunk at `position` in Review's open diff to before the session, as `revert(path:)`
  /// does the whole file: the core's number for the hunk and the digest of the file Review read,
  /// so the core refuses the revert with a Notice if the file changed since. A diff without a
  /// digest or numbered hunks, or a position past its hunks, sends nothing.
  public func revert(hunk position: Int, in review: ReviewState) async throws {
    guard let path = review.selection, let diff = review.diff, let digest = diff.digest,
      diff.hunks.indices.contains(position), let index = diff.hunks[position].index
    else { return }
    _ = try await send(.revertHunk(path: path, toTurn: 1, hunk: index, nowDigest: digest))
  }

  /// The Changes tab's plain Rewind (`ChangesTab.Intent.rewind(checkpoint:)`): code only, DT§5.2's
  /// "Restore code to here" (A101). The timeline keeps all three scopes.
  public func rewind(checkpoint id: String) async throws {
    try await rewind(checkpoint: id, code: true, conversation: false)
  }
}

extension ComposerStore {
  /// Edit and resend (A102, DT§5.2): the prompt fills the draft at once, and the conversation
  /// rewinds to before the prompt's turn, so the resent prompt does not see the old reply. Code
  /// is not restored; that stays the rewind timeline's explicit choice. The core's `toTurn` is
  /// the first turn it undoes, so it is the prompt's own turn. A block that is no prompt does
  /// nothing; the task is the rewind's send, for a caller that waits on it.
  @discardableResult
  public func resend(_ prompt: Block) -> Task<Void, Never>? {
    guard case .user(let text, _) = prompt.kind else { return nil }
    edit(text)
    let rewind = Intent.rewind(toTurn: prompt.turn, code: false, conversation: true)
    return Task {
      do {
        _ = try await session.send(rewind)
      } catch {
        report(error)
      }
    }
  }
}
