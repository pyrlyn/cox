// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The prompt's bubble and its hover actions (T37.23.9's Check): the bubble on `UserBubble`'s
// readable face with the glass sweep and e2, a gap above its tile row, at rest and hovered, in
// light and dark, Frosted; the hover shows the actions over the prompt only; Copy puts the
// prompt's text on the pasteboard and Edit and resend fills the composer.

import AppKit
import CoxClient
import CoxModel
import CoxUI
import SnapshotTesting
import Testing

@testable import CoxTranscript

private let prompt = "Please fix the flaky watcher test.\nIt fails about once in ten runs."

private let turn: [Block] = [
  Block(id: "u", turn: 1, kind: .user(text: prompt, attachments: ["watcher.log"])),
  Block(
    id: "a", turn: 1,
    kind: .assistant(
      text: "Fixed it.",
      doc: StyledDoc(blocks: [
        .text(kind: .paragraph, lines: [TextLine([Span(text: "Fixed it.")])])
      ])
    )),
]

// The reading column's width; the height follows the text.
// swiftlint:disable:next no_literal_size
private let size = NSSize(width: 760, height: 300)

@MainActor
@Suite(.serialized)
struct PromptBubbleTests {
  @Test(arguments: [false, true], [false, true])
  func promptAtRestAndHovered(dark: Bool, hovered: Bool) throws {
    let host = Host(turn, size: size, dark: dark, material: .frosted)
    defer { host.close() }
    host.composer = ComposerStore(session: host.store)
    host.show(crossBlockSelection: true)
    host.fitToText()
    if hovered { host.text.hover(at: host.text.convert(host.point("u", 3), from: nil)) }
    host.settle()
    #expect(host.text.hoveredPrompt == (hovered ? "u" : nil))
    // Anti-aliasing differs slightly between machines; a real change moves far more pixels.
    assertSnapshot(
      of: try host.image(), as: .image(precision: 0.995, perceptualPrecision: 0.98),
      named: "\(dark ? "dark" : "light")-frosted-\(hovered ? "hovered" : "rest")",
      testName: "promptAtRestAndHovered")
  }

  @Test func onlyAPromptShowsItsActionsAndTheyStayWhileThePointerIsOnThem() throws {
    let host = Host(turn, size: size)
    defer { host.close() }
    host.settle()
    let text = host.text
    text.hover(at: text.convert(host.point("u", 3), from: nil))
    #expect(text.hoveredPrompt == "u")
    let strip = try #require(text.subviews.last)
    text.hover(at: NSPoint(x: strip.frame.midX, y: strip.frame.midY))
    #expect(text.hoveredPrompt == "u")
    text.hover(at: text.convert(host.point("a", 3), from: nil))
    #expect(text.hoveredPrompt == nil)
    #expect(!text.subviews.contains(strip))
    text.hover(at: text.convert(host.point("u", 3), from: nil))
    text.hover(at: nil)
    #expect(text.hoveredPrompt == nil)
  }

  @Test func copyPutsThePromptOnThePasteboardAndEditFillsTheComposer() {
    let store = SessionStore(session: FixtureSession(fixture: Fixture(batches: [], snapshot: [])))
    let composer = ComposerStore(session: store)
    let board = NSPasteboard(name: NSPasteboard.Name("cox.transcript.tests.\(UUID())"))
    defer { board.releaseGlobally() }
    var acting = PromptActing(composer: composer)
    acting.pasteboard = board

    acting.perform(.copy, on: turn[0])
    #expect(board.string(forType: .string) == prompt)
    #expect(composer.text.isEmpty)

    acting.perform(.edit, on: turn[0])
    #expect(composer.text == prompt)
  }

  @Test func withoutAComposerAPromptOffersCopyOnly() {
    #expect(PromptActing(composer: nil).offered == [.copy])
  }
}
