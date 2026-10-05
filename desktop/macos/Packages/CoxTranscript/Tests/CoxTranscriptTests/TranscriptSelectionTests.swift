// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The transcript's selection check (T37.23, A67): through the SwiftUI view the app hosts, with
// CoxUI's tool card between the blocks, a drag across three blocks copies all three as
// Markdown in order; with `cross_block_selection` off the same drag selects one block. Also:
// patches the store applies reach the text, and a card block is hosted as one character.

import AppKit
import CoxClient
import CoxTranscript
import Foundation
import Testing

private let userText = "Please fix the **flaky** test in cox-core."
private let summary = "Ran cargo nextest run -p cox-core"
private let replySource = "Fixed it:\n\n```sh\ncargo nextest run -p cox-core\n```"

/// A user message, a finished tool call with its tail, a reply with a code block, a thought.
private let transcript: [Block] = [
  Block(id: "u", turn: 1, kind: .user(text: userText, attachments: [])),
  Block(
    id: "t", turn: 1,
    kind: .tool(
      tool: "bash", summary: summary, icon: .shell, risk: .exec, state: .done,
      tail: "    Starting 38 tests\n        PASS cox-core\n", archive: nil, diff: nil,
      durationMs: 1_200)),
  Block(
    id: "a", turn: 1,
    kind: .assistant(
      text: replySource,
      doc: StyledDoc(blocks: [
        .text(kind: .paragraph, lines: [TextLine([Span(text: "Fixed it:")])]),
        .code(lang: "sh", lines: [[Span(text: "cargo nextest run -p cox-core")]]),
      ]))),
  Block(id: "k", turn: 2, kind: .thinking(text: "Next turn.")),
]

/// The code line's offset inside the reply block.
private let codeOffset = ("Fixed it:\n" as NSString).length + 6

// The reading column's width and room for every block, not design sizes.
// swiftlint:disable:next no_literal_size
private let size = NSSize(width: 760, height: 600)

@MainActor
@Suite(.serialized)
struct TranscriptSelectionTests {
  @Test(.enabled(if: syntheticMouse, "synthesized mouse events need macOS 27"))
  func dragAcrossThreeBlocksCopiesTheirMarkdownInOrder() throws {
    let host = Host(transcript, size: size)
    defer { host.close() }
    host.settle()

    host.drag(from: host.point("u", 7), to: host.point("a", codeOffset))

    #expect(host.selectedBlocks == ["u", "t", "a"])
    let copied = try #require(host.copy().markdown)
    // The code cut short comes back from the doc writer, the plain stand-in here: no fence.
    let parts = [
      "fix the **flaky** test in cox-core.\n\n", summary + "\n\n", "Fixed it:\n\ncargo",
    ]
    let found = try parts.map { try #require(copied.range(of: $0), "\($0) in \(copied)") }
    #expect(found.map(\.lowerBound) == found.map(\.lowerBound).sorted())
    #expect(!copied.contains("Next turn"))
  }

  @Test(.enabled(if: syntheticMouse, "synthesized mouse events need macOS 27"))
  func withTheSettingOffTheSameDragSelectsOneBlock() throws {
    let host = Host(transcript, size: size)
    defer { host.close() }
    host.settle()
    // The app flips the setting on a live view.
    host.show(crossBlockSelection: false)
    #expect(host.text.crossBlockSelection == false)

    host.drag(from: host.point("u", 7), to: host.point("a", codeOffset))

    #expect(host.selectedBlocks == ["u"])
    let markdown = try #require(host.copy().markdown)
    #expect(!markdown.isEmpty && userText.hasSuffix(markdown))
  }

  @Test func patchesTheStoreAppliesReachTheText() throws {
    let host = Host(transcript, size: size)
    defer { host.close() }
    let running = Block(
      id: "r", turn: 2,
      kind: .tool(
        tool: "bash", summary: "Running git status", icon: .shell, risk: .readOnly,
        state: .running, tail: "", archive: nil, diff: nil, durationMs: 0))

    host.text.setThought("k", open: true)
    host.store.apply([
      .appendText(id: "k", text: " Then the docs."), .upsert(block: running, after: "k"),
    ])

    let thought = try #require(host.text.range(of: "k"))
    let shown = (host.text.string as NSString).substring(with: thought)
    #expect(shown == "\u{FFFC}\nNext turn. Then the docs.", "the fold header, then the reasoning")
    #expect(host.text.range(of: "r")?.length == 1, "a card is one attachment character")
    #expect(host.text.blockRanges.ids == ["u", "t", "a", "k", "r"])
  }
}
