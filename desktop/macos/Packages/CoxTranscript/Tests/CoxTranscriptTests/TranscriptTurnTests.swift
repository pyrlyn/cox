// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A turn's prompt and thoughts inside the transcript text (T37.23.4's Check): the prompt on
// `UserBubble`'s face with its attachment's tile, a folded and an open thought as
// `ThinkingDisclosure` reads, in light and dark, Solid; and a drag from the prompt into the reply
// copies both as Markdown in order, through the SwiftUI view the app hosts.

import AppKit
import CoxClient
import SnapshotTesting
import Testing

private let prompt = "Please fix the flaky watcher test.\nIt fails about once in ten runs."
private let replySource = "Fixed it:\n\n```sh\ncargo nextest run -p cox-core\n```"

private let turn: [Block] = [
  Block(id: "u", turn: 1, kind: .user(text: prompt, attachments: ["watcher.log"])),
  Block(id: "k1", turn: 1, kind: .thinking(text: "The log shows a late event.")),
  Block(
    id: "k2", turn: 1,
    kind: .thinking(text: "The test sleeps instead of waiting.\nWait for the first event.")),
  Block(
    id: "a", turn: 1,
    kind: .assistant(
      text: replySource,
      doc: StyledDoc(blocks: [
        .text(kind: .paragraph, lines: [TextLine([Span(text: "Fixed it:")])]),
        .code(lang: "sh", lines: [[Span(text: "cargo nextest run -p cox-core")]]),
      ]))),
]

// The reading column's width; the height follows the text.
// swiftlint:disable:next no_literal_size
private let size = NSSize(width: 760, height: 400)

@MainActor
@Suite(.serialized)
struct TranscriptTurnTests {
  @Test(arguments: [false, true])
  func promptWithAnAttachmentAndAFoldedAndAnOpenThought(dark: Bool) throws {
    let host = Host(turn, size: size, dark: dark)
    defer { host.close() }
    host.text.setThought("k2", open: true)
    host.fitToText()
    // Anti-aliasing differs slightly between machines; a real change moves far more pixels.
    assertSnapshot(
      of: try host.image(), as: .image(precision: 0.995, perceptualPrecision: 0.98),
      named: dark ? "dark-solid" : "light-solid",
      testName: "promptWithAnAttachmentAndAFoldedAndAnOpenThought")
  }

  @Test(.enabled(if: syntheticMouse, "synthesized mouse events need macOS 27"))
  func dragFromThePromptIntoTheReplyCopiesBothAsMarkdownInOrder() throws {
    let host = Host(turn, size: size)
    defer { host.close() }
    host.text.setThought("k2", open: true)
    host.settle()

    host.drag(from: host.point("u", 7), to: host.point("a", 4))

    #expect(host.selectedBlocks == ["u", "k1", "k2", "a"])
    let copied = try #require(host.copy().markdown)
    // The prompt without its tile, then the open thought's reasoning (a folded one shows no
    // text), then the reply up to the drag's end.
    let expected = [
      "fix the flaky watcher test.\nIt fails about once in ten runs.",
      "The test sleeps instead of waiting.\nWait for the first event.", "Fix",
    ]
    #expect(copied.hasPrefix(expected.joined(separator: "\n\n")), "\(copied)")
  }
}
