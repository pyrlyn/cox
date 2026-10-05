// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The rewind timeline's intent (T37.28.1): a Changes tab checkpoint and a scope reach the session
// as `Intent.rewind` to that turn; a file's Revert (T37.28.3) as `Intent.revertFile`; a Review
// hunk's Revert hunk (T51.21) as `Intent.revertHunk`.

import CoxClient
import Testing

@testable import CoxModel

@MainActor
@Test func aCheckpointRewindsToItsTurnInThePickedScope() async throws {
  let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []))
  let store = SessionStore(session: session)
  let changes = Changes(checkpoints: [Checkpoint(turn: 2, label: "Turn 2", time: "")])
  let id = try #require(ChangesTabState(changes).checkpoints.first?.id)
  try await store.rewind(checkpoint: id, code: true, conversation: false)
  try await store.rewind(checkpoint: id, code: true, conversation: true)
  try await store.rewind(checkpoint: "not a turn", code: true, conversation: true)
  #expect(
    session.sent == [
      .rewind(toTurn: 2, code: true, conversation: false),
      .rewind(toTurn: 2, code: true, conversation: true),
    ])
}

@MainActor
@Test func aFilesRevertRestoresItToBeforeTheSession() async throws {
  let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []))
  let store = SessionStore(session: session)
  try await store.revert(path: "src/retry.rs")
  #expect(session.sent == [.revertFile(path: "src/retry.rs", toTurn: 1)])
}

/// Review's open diff with two hunks the core numbered 0 and 1, read when the file hashed `digest`.
private func reviewed(digest: String?) -> ReviewState {
  let hunks = [
    DiffHunk(header: "@@ -1 +1 @@", lines: [], index: 0),
    DiffHunk(header: "@@ -9 +9 @@", lines: [], index: 1),
  ]
  return ReviewState(
    Changes(), selection: "src/retry.rs",
    diff: DiffModel(path: "/w/src/retry.rs", hunks: hunks, digest: digest))
}

@MainActor
@Test func revertHunkSendsTheHunksIndexAndTheDiffsDigest() async throws {
  let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []))
  let store = SessionStore(session: session)
  try await store.revert(hunk: 1, in: reviewed(digest: "00ff00ff00ff00ff"))
  #expect(
    session.sent == [
      .revertHunk(path: "src/retry.rs", toTurn: 1, hunk: 1, nowDigest: "00ff00ff00ff00ff")
    ])
}

@MainActor
@Test func revertHunkSendsNothingWithoutADigestOrPastTheHunks() async throws {
  let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []))
  let store = SessionStore(session: session)
  try await store.revert(hunk: 0, in: reviewed(digest: nil))
  try await store.revert(hunk: 2, in: reviewed(digest: "00ff00ff00ff00ff"))
  try await store.revert(hunk: 0, in: ReviewState())
  #expect(session.sent.isEmpty)
}

@MainActor
@Test func theChangesTabsPlainRewindRestoresCodeOnly() async throws {
  let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []))
  try await SessionStore(session: session).rewind(checkpoint: "3")
  #expect(session.sent == [.rewind(toTurn: 3, code: true, conversation: false)])
}

/// Edit and resend on the second prompt of `two-prompts.json` (T37.23.18's Check): the prompt
/// fills the draft, and one conversation-only rewind undoes its turn — the prompt and its reply —
/// while the first prompt's turn stays.
@MainActor
@Test func editAndResendFillsTheComposerAndRewindsTheConversationToBeforeThatPrompt() async throws {
  let url = try #require(fixtures.first { $0.lastPathComponent == "two-prompts.json" })
  let session = FixtureSession(fixture: try Fixture(contentsOf: url))
  let composer = ComposerStore(session: SessionStore(session: session))
  await composer.session.run()
  let blocks = Array(composer.session.blocks.values)
  let prompts = blocks.filter { if case .user = $0.kind { true } else { false } }
  try #require(prompts.count == 2)
  let (first, second) = (prompts[0], prompts[1])
  guard case .user(let text, _) = second.kind else { return }

  await composer.resend(second)?.value

  #expect(composer.text == text)
  #expect(session.sent == [.rewind(toTurn: second.turn, code: false, conversation: true)])
  // The core undoes `toTurn` itself: what goes is the second prompt and its reply.
  let undone = blocks.filter { $0.turn >= second.turn }
  #expect(undone.contains(second) && !undone.contains(first))
  #expect(undone.contains { if case .assistant = $0.kind { true } else { false } })
}

@MainActor
@Test func resendIgnoresABlockThatIsNoPrompt() {
  let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []))
  let composer = ComposerStore(session: SessionStore(session: session))
  let reply = Block(id: "a", turn: 1, kind: .assistant(text: "hi", doc: .init(blocks: [])))
  #expect(composer.resend(reply) == nil)
  #expect(composer.text.isEmpty)
  #expect(session.sent.isEmpty)
}
