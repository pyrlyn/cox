// Figma sync seams (frames 22-empty-session and 14-rewind-edit-resend): a welcome suggestion
// drafts its prompt; the rewind menu counts the files changed at or after its turn (T37.46), a
// failed preview leaves the count unknown, and its scopes and fork reach the core as
// `Intent.rewind` and `Intent.fork`.

import CoxClient
import Testing

@testable import CoxModel

private struct FailingPreview: RewindPreviewService {
  struct Failure: Error {}
  func restoredFiles(beforeTurn turn: UInt32) async throws -> Int { throw Failure() }
}

@MainActor
@Test func aWelcomeSuggestionDraftsItsPrompt() async throws {
  let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []))
  let composer = ComposerStore(session: SessionStore(session: session))
  let facts = try await FixtureWelcome(
    WelcomeFacts(
      summary: "Rust workspace · 31 crates · AGENTS.md loaded",
      suggestions: [
        WelcomeSuggestion(
          title: "Explain the architecture", detail: "How do the parts of cox fit together?",
          prompt: "Explain the architecture: how do the parts of cox fit together?"),
        WelcomeSuggestion(
          title: "Find and fix a failing test",
          detail: "Run cargo nextest and fix the first failure",
          prompt: "Run cargo nextest and fix the first failure."),
      ])
  ).welcome(cwd: "/w/cox")
  let first = try #require(facts.suggestions.first)
  composer.suggest(first)
  #expect(composer.text == first.prompt)
  #expect(session.sent.isEmpty, "a suggestion drafts; the person sends")
}

/// One file per call; `turn` is the turn that changed it last.
private func file(_ path: String, turn: UInt32) -> ChangedFile {
  ChangedFile(path: path, change: .edited, added: 1, removed: 0, call: path, turn: turn)
}

@MainActor
@Test func theRewindMenuCountsTheFilesChangedAtOrAfterItsTurn() async {
  let changes = Changes(turns: [
    TurnFiles(turn: 1, files: [file("Cargo.toml", turn: 1)]),
    TurnFiles(turn: 3, files: [file("src/retry.rs", turn: 3), file("tests/backoff.rs", turn: 3)]),
  ])
  let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []), changes: changes)
  let store = SessionStore(session: session)
  var counts: [Int?] = []
  for turn: UInt32 in [1, 2, 3, 4] {
    counts.append(await store.rewindMenu(turn: turn).restoredFiles)
  }
  #expect(counts == [3, 2, 2, 0])
}

@MainActor
@Test func aFailedPreviewLeavesTheCountUnknown() async {
  let store = SessionStore(session: FixtureSession(fixture: Fixture(batches: [], snapshot: [])))
  #expect(
    await store.rewindMenu(turn: 2, preview: FailingPreview())
      == RewindMenuState(turn: 2, restoredFiles: nil))
}

@MainActor
@Test func theRewindMenuRewindsAndForksBeforeItsTurn() async throws {
  let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []))
  let store = SessionStore(session: session)
  try await store.rewind(toTurn: 2, code: true, conversation: true)
  try await store.fork(beforeTurn: 2)
  #expect(session.sent == [.rewind(toTurn: 2, code: true, conversation: true), .fork(turn: 2)])
}
