// A prompt's rewind menu from its turn number (T37.48's Check, Figma frame 14): hovering a
// prompt in a window wider than the reading column floats the marked number over the drawn one,
// the pointer on it keeps the hover, it opens the menu in a popover, and the menu's scopes and
// Fork reach the session as the core's intents.

import AppKit
import CoxClient
import CoxModel
import Testing

@testable import CoxTranscript
@testable import CoxTranscriptText
@testable import CoxUI

private let turns: [Block] = [
  Block(id: "u1", turn: 1, kind: .user(text: "Fix the watcher.", attachments: [])),
  Block(id: "u2", turn: 2, kind: .user(text: "Now add a test.", attachments: [])),
]

// Wider than the reading column, so its margin holds the gutter.
// swiftlint:disable:next no_literal_size
private let size = NSSize(width: 1_000, height: 300)

@MainActor
@Suite(.serialized)
struct PromptRewindTests {
  @Test func aHoveredPromptsTurnNumberOpensItsRewindMenu() throws {
    let host = Host(turns, size: size)
    defer { host.close() }
    host.settle()
    let text = host.text
    text.hover(at: text.convert(host.point("u2", 3), from: nil))
    #expect(text.hoveredPrompt == "u2")
    let box = try #require(text.gutterBox("u2"))
    #expect(box.minX >= 0, "the gutter is inside the view")
    let gutter = try #require(text.promptHover?.gutter)
    #expect(gutter.frame.intersects(box), "over the drawn number")
    text.hover(at: NSPoint(x: gutter.frame.midX, y: gutter.frame.midY))
    #expect(text.hoveredPrompt == "u2", "the pointer on the number keeps the hover")

    text.openMenu("u2")
    #expect(text.promptMenu != nil)
    text.promptMenu?.close()
  }

  @Test func theMenusScopesAndForkReachTheSession() async {
    let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []))
    let store = SessionStore(session: session)
    let acting = PromptActing(composer: nil, store: store)
    await acting.perform(.rewind(turn: 2, code: true, conversation: false))?.value
    await acting.perform(.rewind(turn: 2, code: false, conversation: true))?.value
    await acting.perform(.fork(turn: 2))?.value
    #expect(
      session.sent == [
        .rewind(toTurn: 2, code: true, conversation: false),
        .rewind(toTurn: 2, code: false, conversation: true), .fork(turn: 2),
      ])
    #expect(acting.gutter(for: turns[1]) {}.isMarked)
  }

  @Test func withoutAStoreTheMenuSendsNothing() {
    #expect(PromptActing(composer: nil).perform(.fork(turn: 1)) == nil)
  }
}
