// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The transcript's look (T37.23, DS§6.4 row `TranscriptView`): one transcript with every block
// kind — prose, code, a thought, tool calls running, done and failed, a group, a subagent task,
// an approval and a question in the slot, a compaction, a checkpoint, a notice and an error —
// in light and dark, Solid (DS§9), through the SwiftUI view the app hosts.

import AppKit
import CoxClient
import CoxUI
import SnapshotTesting
import Testing

private func line(_ text: String) -> [Span] { [Span(text: text)] }

private let reply = StyledDoc(blocks: [
  .text(kind: .heading(2), lines: [TextLine(line("The fix"))]),
  .text(
    kind: .paragraph,
    lines: [TextLine(line("The test raced the watcher; it now waits for the event."))]),
  .text(
    kind: .list,
    lines: [TextLine(line("wait for the first event")), TextLine(line("drop the sleep"))]),
  .code(
    lang: "rust",
    lines: [line("let event = rx.recv().await?;"), line("assert_eq!(event.kind, Kind::Write);")]),
  .text(kind: .quote, lines: [TextLine(line("Flaky no more."))]),
])

private let everyKind: [Block] = [
  Block(id: "u", turn: 1, kind: .user(text: "Please fix the flaky watcher test.", attachments: [])),
  Block(id: "k", turn: 1, kind: .thinking(text: "The test sleeps instead of waiting.")),
  Block(
    id: "g", turn: 1,
    kind: .toolGroup(summary: "Explored 3 files", children: ["r1", "r2", "r3"], state: .done)),
  Block(
    id: "e", turn: 1,
    kind: .tool(
      tool: "edit", summary: "Edited watcher.rs", icon: .edit, risk: .write, state: .done, tail: "",
      archive: nil, diff: nil, durationMs: 40)),
  Block(
    id: "f", turn: 1,
    kind: .tool(
      tool: "bash", summary: "Ran cargo nextest run", icon: .shell, risk: .exec, state: .failed,
      tail: "    Starting 12 tests\n        FAIL watcher_sees_write\n", archive: nil, diff: nil,
      durationMs: 3_400)),
  Block(
    id: "x", turn: 1,
    kind: .tool(
      tool: "bash", summary: "Running rm -rf target", icon: .shell, risk: .destructive,
      state: .running, tail: "removing target/debug\n", archive: nil, diff: nil, durationMs: 0)),
  Block(
    id: "t", turn: 1,
    kind: .task(
      task: "t1", label: "Review the fix", tier: .cheap, done: true, costUsd: 0.01, exitCode: 0,
      state: .succeeded, kind: .agent)),
  Block(
    id: "p", turn: 1,
    kind: .approval(
      call: "c1", tool: "bash", summary: "git push", input: #"{"command":"git push"}"#,
      grants: ["git push"], why: .risk(risk: .exec), source: nil, decision: nil, by: nil)),
  Block(
    id: "q", turn: 1,
    kind: .question(call: "c2", question: "Keep the sleep as a fallback?", options: [], answer: nil)
  ),
  Block(id: "a", turn: 1, kind: .assistant(text: "", doc: reply)),
  Block(
    id: "c", turn: 2,
    kind: .compaction(
      beforeTokens: 180_000, afterTokens: 24_000, reason: .postTurn,
      summary: "Compacted 180k tokens to 24k.")),
  Block(id: "h", turn: 2, kind: .checkpoint(files: ["src/watcher.rs"])),
  Block(id: "n", turn: 2, kind: .notice(level: .warn, text: "Half the budget is spent.")),
  Block(id: "r", turn: 2, kind: .error(text: "The provider closed the stream.", fatal: false)),
]

// The reading column's width; the height follows the text.
// swiftlint:disable:next no_literal_size
private let size = NSSize(width: 760, height: 400)

@MainActor
@Suite(.serialized)
struct TranscriptSnapshotTests {
  @Test(arguments: [false, true])
  func everyBlockKind(dark: Bool) throws {
    let host = Host(everyKind, size: size, dark: dark)
    defer { host.close() }
    host.fitToText()
    // Anti-aliasing differs slightly between machines; a real change moves far more pixels.
    assertSnapshot(
      of: try host.image(), as: .image(precision: 0.995, perceptualPrecision: 0.98),
      named: dark ? "dark-solid" : "light-solid", testName: "everyBlockKind")
  }
}
