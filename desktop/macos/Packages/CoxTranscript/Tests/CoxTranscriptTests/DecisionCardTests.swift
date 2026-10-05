// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The waiting cards in the transcript (T37.27, DT§5.2): an approval or question block fills
// its CoxUI card with the words DT§5.2 gives, a decided one shrinks to its line, a choice
// becomes the intent the core expects, and the transcript draws the real cards in its slot.

import AppKit
import CoxClient
import CoxUI
import SnapshotTesting
import Testing

@testable import CoxTranscript

private func approval(
  _ id: BlockID, tool: String = "bash", why: Why, source: Source? = nil,
  decision: Decision? = nil, by decider: DecidedBy? = nil
) -> Block {
  Block(
    id: id, turn: 1,
    kind: .approval(
      call: "call-\(id)", tool: tool, summary: "git push -u origin main",
      input: #"{"command":"git push -u origin main"}"#, grants: ["git push -u origin main"],
      why: why, source: source, decision: decision, by: decider))
}

private func question(_ id: BlockID, options: [String] = [], answer: String? = nil) -> Block {
  Block(
    id: id, turn: 1,
    kind: .question(
      call: "call-\(id)", question: "Keep the sleep as a fallback?", options: options,
      answer: answer))
}

@Test func aPendingApprovalFillsItsCardWithWhatWhyAndWho() throws {
  let block = approval(
    "p", why: .ruleAsk(rule: "Bash(git push:*)"),
    source: Source(session: "child", agent: "reviewer", preset: nil))
  let content = try #require(ApprovalCard.Content(block))
  #expect(content.title == "Run this command?")
  #expect(content.command == "git push -u origin main")
  #expect(content.reason == "matches ask rule Bash(git push:*)")
  #expect(content.source == "reviewer")
  #expect(content.risk == nil)
  #expect(content.outcome == nil)
}

@Test func onlyADestructiveRiskCarriesAChip() throws {
  let risky = try #require(ApprovalCard.Content(approval("d", why: .risk(risk: .destructive))))
  #expect(risky.risk?.level == .high)
  #expect(risky.reason == "risk: destructive")
  let write = try #require(
    ApprovalCard.Content(approval("w", tool: "write", why: .risk(risk: .write))))
  #expect(write.risk == nil)
  #expect(write.title == "Allow write?")
}

@Test func aDecidedApprovalShrinksToHowItWasDecided() throws {
  let why = Why.risk(risk: .exec)
  let session = ApprovalCard.Content(
    approval("s", why: why, decision: .allowForSession, by: .user))
  #expect(session?.outcome == .init(text: "Allowed by you · for session", isAllowed: true))
  let denied = ApprovalCard.Content(
    approval("n", why: why, decision: .deny(reason: "no"), by: .rule))
  #expect(denied?.outcome == .init(text: "Denied by a rule", isAllowed: false))
}

@Test func eachButtonSendsTheDecisionTheCoreExpects() {
  #expect(ApprovalCard.Action.allow.decision == .allow)
  #expect(ApprovalCard.Action.allowForSession.decision == .allowForSession)
  #expect(ApprovalCard.Action.deny.decision == .deny(reason: "denied by user"))
}

@Test func anApprovalShowsWhatAllowForSessionGrantsAndItsInputToEdit() throws {
  let content = try #require(ApprovalCard.Content(approval("g", why: .risk(risk: .exec))))
  #expect(content.grant == "bash: git push -u origin main")
  #expect(content.input == "{\n  \"command\" : \"git push -u origin main\"\n}")
  #expect(
    ApprovalCard.Content.grant("bash", ["git status", "npm test"])
      == "bash: git status · npm test")
  #expect(ApprovalCard.Content.grant("bash", []) == nil)
  #expect(ApprovalCard.Content.editable("null") == nil)
}

@MainActor
@Test func anEditSendsTheEditedJSONAsTheDecision() {
  var sent: [Intent] = []
  let card = DecisionCard(block: approval("e", why: .risk(risk: .exec))) { sent.append($0) }
  card.edit(#"{"command": "git push origin main"}"#)
  #expect(
    sent == [
      .approve(call: "call-e", decision: .edit(input: #"{"command": "git push origin main"}"#))
    ])
}

@Test func aQuestionFillsItsCardAndAnAnswerCollapsesIt() throws {
  let pending = try #require(QuestionCard.Content(question("q", options: ["yes", "no"])))
  #expect(pending.options == ["yes", "no"])
  #expect(pending.answer == nil)
  #expect(QuestionCard.Content(question("a", answer: "no"))?.answer == "no")
  #expect(QuestionCard.Content(approval("p", why: .risk(risk: .exec))) == nil)
}

// The reading column's width; the height follows the text.
// swiftlint:disable:next no_literal_size
private let size = NSSize(width: 760, height: 400)

@MainActor
@Suite(.serialized)
struct DecisionCardSnapshotTests {
  @Test(arguments: [false, true])
  func pendingAndDecidedCards(dark: Bool) throws {
    let blocks = [
      Block(id: "u", turn: 1, kind: .user(text: "Push the fix.", attachments: [])),
      approval("p", why: .ruleAsk(rule: "Bash(git push:*)")),
      approval("s", why: .risk(risk: .exec), decision: .allowForSession, by: .user),
      question("q", options: ["Keep it", "Drop it"]),
      question("a", answer: "Drop it"),
    ]
    let host = Host(blocks, size: size, dark: dark) { _ in }
    defer { host.close() }
    host.fitToText()
    // Anti-aliasing differs slightly between machines; a real change moves far more pixels.
    assertSnapshot(
      of: try host.image(), as: .image(precision: 0.995, perceptualPrecision: 0.98),
      named: dark ? "dark-solid" : "light-solid", testName: "pendingAndDecidedCards")
  }
}
