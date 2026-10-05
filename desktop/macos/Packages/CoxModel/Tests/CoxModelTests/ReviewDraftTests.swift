// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Review's line comments (T37.28.4): a click anchors the draft at the line's number, a removed
// line at its old number, and Send posts the whole draft as one `Intent.send` and empties it;
// while a turn runs it queues, unless `[desktop.review] send = "now"` (A108).

import CoxClient
import Testing

@testable import CoxModel

private let retry = ReviewState(
  Changes(), selection: "src/retry.rs",
  diff: DiffModel(
    path: "src/retry.rs",
    hunks: [
      DiffHunk(
        header: "@@ -40,3 +40,3 @@",
        lines: [
          DiffLine(kind: .context, old: 40, new: 40, spans: []),
          DiffLine(kind: .del, old: 41, new: nil, spans: []),
          DiffLine(kind: .add, old: nil, new: 41, spans: []),
        ])
    ]))

@Test func aClickAnchorsAtTheLineOnDiskOrAtTheRemovedLinesOldNumber() {
  var draft = ReviewDraft()
  draft.pick(retry, hunk: 0, line: 2)
  draft.save("  Use saturating_mul.  ")
  draft.pick(retry, hunk: 0, line: 1)
  draft.save("Why drop this?")
  #expect(
    draft.comments == [
      LineComment(path: "src/retry.rs", line: 41, text: "Use saturating_mul."),
      LineComment(path: "src/retry.rs", line: 41, removed: true, text: "Why drop this?"),
    ])
  #expect(draft.editing == nil)
}

@Test func blankTextOrALineOutsideTheDiffAddsNothing() {
  var draft = ReviewDraft()
  draft.pick(retry, hunk: 1, line: 0)
  #expect(draft.editing == nil)
  draft.pick(ReviewState(), hunk: 0, line: 0)
  #expect(draft.editing == nil)
  draft.pick(retry, hunk: 0, line: 0)
  #expect(draft.editing == LineComment(path: "src/retry.rs", line: 40, text: ""))
  draft.save(" \n ")
  #expect(draft == ReviewDraft())
}

@MainActor
@Test func sendPostsTheDraftAsOneTurnAndEmptiesIt() async throws {
  let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []))
  let store = SessionStore(session: session)
  try await store.sendReview()
  #expect(session.sent.isEmpty)
  store.reviewDraft = ReviewDraft(comments: [
    LineComment(path: "a.rs", line: 1, text: "One."),
    LineComment(path: "b.rs", line: 2, text: "Two."),
  ])
  try await store.sendReview()
  #expect(session.sent == [.send(text: "a.rs:1 One.\nb.rs:2 Two.", attachments: [])])
  #expect(store.reviewDraft == ReviewDraft())
}

@MainActor
@Test func whileATurnRunsSendQueuesByDefaultAndSendsAtOnceWithNow() async throws {
  let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []))
  let store = SessionStore(session: session)
  let tally = Tally(
    sent: 0, received: 0, cacheRead: 0, cacheWrite: 0, uncached: 0, costUsd: 0, calls: 0,
    estimated: false)
  let turn = TurnUsage(
    turn: "t", tally: tally, thinkingTokens: 0, ttftMs: nil, tokPerS: nil, exact: false,
    sparkline: [], done: false)
  store.apply([.usage(usage: UsageView(session: tally, turn: turn, contextTokens: 0))])
  let comment = [LineComment(path: "a.rs", line: 1, text: "One.")]
  store.reviewDraft = ReviewDraft(comments: comment)
  try await store.sendReview()
  store.reviewDraft = ReviewDraft(comments: comment)
  try await store.sendReview(.now)
  #expect(
    session.sent == [
      .queue(text: "a.rs:1 One.", attachments: []), .send(text: "a.rs:1 One.", attachments: []),
    ])
  #expect(store.reviewDraft == ReviewDraft())
}

@MainActor
@Test func theReviewSendSettingReadsBackFromTheSettings() async {
  let view = SettingsView(
    settings: [
      Setting(
        key: "desktop.review.send", value: "\"now\"", layer: .user, editable: true,
        kind: .choice(options: ["queue", "now"]), description: "")
    ],
    userFile: "/home/.cox/config.toml")
  let store = SettingsStore(
    client: FixtureSettingsClient(view: view), secrets: MemorySecretStore(), cwd: "/project")
  #expect(store.reviewSend == .queue)
  await store.load()
  #expect(store.reviewSend == .now)
  await store.set("desktop.review.send", to: .text("queue"))
  #expect(store.reviewSend == .queue)
}
