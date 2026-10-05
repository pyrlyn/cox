// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// ComposerStore (T37.24): the draft asks the client for rows only for an `@` token or a leading
// `/` token, a picked row replaces the token, and each kind of draft leaves as its one intent.

import CoxClient
import Foundation
import Testing

@testable import CoxModel

@MainActor
private func composer() -> (ComposerStore, FixtureSession) {
  let rows = [
    Completion(insert: "@src/lib.rs", detail: "src/lib.rs"),
    Completion(insert: "@src/main.rs", detail: "src/main.rs"),
    Completion(insert: "/compact", detail: "/compact [focus]"),
  ]
  let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []), completions: rows)
  return (ComposerStore(session: SessionStore(session: session)), session)
}

@MainActor
@Test func anAtTokenOffersFilesAndAPickedFileIsMentionedAndSent() async {
  let (store, session) = composer()
  store.edit("look at @ma")
  #expect(store.completions.map(\.insert) == ["@src/main.rs"])

  store.pick(0)
  #expect(store.text == "look at @src/main.rs ")
  #expect(store.mentions == ["@src/main.rs"])
  #expect(store.completions.isEmpty)

  await store.submit()
  #expect(session.sent == [.send(text: "look at @src/main.rs ", attachments: [])])
  #expect(store.text.isEmpty && store.mentions.isEmpty)
}

/// T37.24.9: the token is the word the caret ends, wherever it stands; the offsets are UTF-16, so
/// the `é` before it counts once.
@MainActor
@Test func theTokenAtTheCaretIsCompletedMidTextAndTheRestStays() {
  let (store, _) = composer()
  store.edit("café @ma and more")
  #expect(store.completions.isEmpty)
  store.select(8..<8)
  #expect(store.completions.map(\.insert) == ["@src/main.rs"])

  store.pick(0)
  #expect(store.text == "café @src/main.rs and more")
  #expect(store.selectedRange == 18..<18)
  #expect(store.mentions == ["@src/main.rs"])

  store.select(6..<6)
  #expect(store.completions.isEmpty)
  store.select(5..<9)
  #expect(store.completions.isEmpty)
  store.select(26..<26)
  #expect(store.selectedRange == nil)
}

@MainActor
@Test func aSlashTokenOffersCommandsOnlyAsTheFirstWord() async {
  let (store, session) = composer()
  store.edit("/co")
  #expect(store.completions.map(\.insert) == ["/compact"])
  store.edit("fix /co")
  #expect(store.completions.isEmpty)

  store.edit("/compact")
  await store.submit()
  #expect(session.sent == [.command(line: "/compact")])
}

@MainActor
@Test func aBangIntoAnEmptyDraftEntersShellModeAndSendsAShellLine() async {
  let (store, session) = composer()
  store.edit("!")
  #expect(store.isShell && store.text.isEmpty)
  store.edit("git status @src")
  #expect(store.completions.isEmpty)
  store.shareOutput = false

  await store.submit()
  #expect(session.sent == [.shell(command: "git status @src", share: false)])
  #expect(!store.isShell)
}

/// T37.44.13: a command or file picked in the palette lands at the end of the draft, a file as a
/// mention, with nothing offered after it.
@MainActor
@Test func aPaletteCommandOrFileIsAppendedToTheDraft() {
  let (store, _) = composer()
  store.append("/compact")
  #expect(store.text == "/compact ")
  store.edit(store.text + "about")
  store.append("@src/lib.rs")
  #expect(store.text == "/compact about @src/lib.rs ")
  #expect(store.mentions == ["@src/lib.rs"])
  #expect(store.completions.isEmpty)
}

/// T37.44.13: without the core's ranking a session still offers the window's rows whose title
/// holds the query, then its commands and files for it.
@MainActor
@Test func theDefaultPaletteKeepsMatchingRowsAndAddsCommandsAndFiles() {
  let (_, session) = composer()
  let items = [
    PaletteItem(kind: .action, id: "review", title: "Review changes"),
    PaletteItem(kind: .action, id: "new", title: "New session"),
  ]
  #expect(session.palette("", items: items, limit: 5).map(\.item.id) == ["review", "new"])
  #expect(session.palette("rev", items: items, limit: 5).map(\.matched) == [[0, 1, 2]])
  let hits = session.palette("ma", items: items, limit: 5)
  #expect(hits.map(\.item.kind) == [.command, .file])
}

@MainActor
@Test func removingAMentionTakesItOutOfTheDraft() {
  let (store, _) = composer()
  store.edit("@li")
  store.moveSelection(by: 1)
  #expect(store.selection == 0)
  store.pick(0)
  store.edit(store.text + "please")
  store.removeMention("@src/lib.rs")
  #expect(store.text == "please")
  #expect(store.mentions.isEmpty)
}

@MainActor
@Test func attachedFilesAreReadAndSentWithTheTurn() async throws {
  let (store, session) = composer()
  let dir = FileManager.default.temporaryDirectory.appending(path: "cox-t37.24-\(UUID())")
  try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
  defer { try? FileManager.default.removeItem(at: dir) }
  let log = dir.appending(path: "trace.txt")
  try Data("429 after 3 tries\n".utf8).write(to: log)

  await store.attach([log, dir.appending(path: "missing.png")])
  #expect(store.failure != nil)
  #expect(store.canSend)
  await store.submit()

  let sent = Attachment(
    name: "trace.txt", mediaType: "text/plain",
    dataB64: Data("429 after 3 tries\n".utf8).base64EncodedString())
  #expect(session.sent == [.send(text: "", attachments: [sent])])
  #expect(store.attachments.isEmpty)
}

/// A usage view of a turn that runs (`done` false) or finished.
private func usage(done: Bool) -> UsageView {
  let tally = Tally(
    sent: 0, received: 0, cacheRead: 0, cacheWrite: 0, uncached: 0, costUsd: 0, calls: 0,
    estimated: false)
  let turn = TurnUsage(
    turn: "t", tally: tally, thinkingTokens: 0, ttftMs: nil, tokPerS: nil, exact: false,
    sparkline: [], done: done)
  return UsageView(session: tally, turn: turn, contextTokens: 0)
}

@MainActor
@Test func whileATurnRunsReturnQueuesAndTheCountIsTheCoresStatus() async {
  let (store, session) = composer()
  store.session.apply([.usage(usage: usage(done: false))])
  #expect(store.isRunning)

  store.edit("next")
  await store.submit()
  store.edit("after that")
  await store.submit()
  #expect(
    session.sent == [
      .queue(text: "next", attachments: []), .queue(text: "after that", attachments: []),
    ])
  #expect(store.queued == 0, "the count is the core's, not the store's")

  store.session.apply([.status(status: Status(queued: 2))])
  #expect(store.queued == 2)
  store.session.apply([.status(status: Status(queued: 1))])
  #expect(store.queued == 1)
  store.session.apply([.usage(usage: usage(done: true))])
  #expect(!store.isRunning)
}

@MainActor
@Test func aDraftWithAttachmentsQueuesWithThemAndCommandReturnSendsNow() async throws {
  let (store, session) = composer()
  store.session.apply([.usage(usage: usage(done: false))])
  let file = FileManager.default.temporaryDirectory.appending(path: "cox-t37.24-\(UUID()).txt")
  try Data("x".utf8).write(to: file)
  defer { try? FileManager.default.removeItem(at: file) }
  let sent = Attachment(
    name: file.lastPathComponent, mediaType: "text/plain",
    dataB64: Data("x".utf8).base64EncodedString())

  await store.attach([file])
  store.edit("look")
  await store.submit()
  #expect(session.sent == [.queue(text: "look", attachments: [sent])])
  #expect(store.attachments.isEmpty && store.failure == nil)

  await store.attach([file])
  store.edit("now")
  await store.submitNow()
  #expect(session.sent.suffix(2) == [.interrupt, .send(text: "now", attachments: [sent])])
  #expect(store.attachments.isEmpty)
}

@MainActor
@Test func upWalksOlderPromptsStopsAtTheOldestAndDownPastTheNewestEmptiesTheDraft() {
  let session = FixtureSession(
    fixture: Fixture(batches: [], snapshot: []), prompts: ["second", "first"])
  let store = ComposerStore(session: SessionStore(session: session))
  store.edit("draft")
  store.recall(-1)
  #expect(store.text == "draft" && !store.isRecalling)

  store.edit("")
  store.recall(-1)
  store.recall(-1)
  store.recall(-1)
  #expect(store.text == "first" && store.isRecalling)
  store.recall(1)
  #expect(store.text == "second")
  store.recall(1)
  #expect(store.text.isEmpty && !store.isRecalling)

  store.recall(-1)
  store.edit("second, edited")
  #expect(!store.isRecalling)
}

/// T37.24.7: the chips show the mode and the model with its effort as the core's status reports
/// them, and ⇧⇥ asks for the mode the core named next, leaving the chip to the core's answer.
@MainActor
@Test func theStatusNamesTheModeAndModelAndCycleAsksForTheCoresNextMode() async {
  let (store, session) = composer()
  #expect(store.mode == nil && store.model == nil)
  await store.cycleMode()
  #expect(session.sent.isEmpty)

  let status = Status(mode: .default, nextMode: .plan, model: "claude-sonnet-5", effort: .high)
  store.session.apply([.status(status: status)])
  #expect(store.mode == .default)
  #expect(store.model == "claude-sonnet-5 · high")
  await store.cycleMode()
  #expect(session.sent == [.setMode(mode: .plan)])
  #expect(store.mode == .default)
}

/// T58.4.14 (A129): the chip is the core's `shortName`, else the id; `modelName` is not shortened.
@MainActor
@Test func theModelChipUsesTheCoresShortNameElseTheId() {
  let (store, _) = composer()
  store.session.apply([
    .status(status: Status(model: "claude-sonnet-5", effort: .high, modelName: "Claude Sonnet 5"))
  ])
  #expect(store.model == "claude-sonnet-5 · high")
  store.session.apply([
    .status(status: Status(model: "claude-sonnet-5", effort: .high, shortName: "Sonnet 5"))
  ])
  #expect(store.model == "Sonnet 5 · high")
}

/// T37.24.10 (A103): the think toggle sends one turn with `confirmThink` — sent now or queued —
/// then turns itself off; a `/` command line is not a turn, so the toggle waits for one.
@MainActor
@Test func theThinkToggleConfirmsOneTurnThenTurnsItselfOff() async {
  let (store, session) = composer()
  store.toggleThink()
  store.edit("/compact")
  await store.submit()
  #expect(store.think, "a command line is not the turn the toggle is for")

  store.edit("plan the refactor")
  await store.submit()
  store.edit("then do it")
  await store.submit()
  #expect(!store.think)

  store.session.apply([.usage(usage: usage(done: false))])
  store.toggleThink()
  store.edit("and review it")
  await store.submit()
  #expect(
    session.sent == [
      .command(line: "/compact"),
      .send(text: "plan the refactor", attachments: [], confirmThink: true),
      .send(text: "then do it", attachments: []),
      .queue(text: "and review it", attachments: [], confirmThink: true),
    ])
  #expect(!store.think)
}
