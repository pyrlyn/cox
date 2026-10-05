// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// T37.24's Check: a `SessionComposer` in a window, driven by real key events — type `@`, pick a
// file from the rows with ↓ and ⏎, type the rest, send with ⏎ — and the intent reaches the
// fixture client, which answers the completion without Rust. T37.24.6's: ↑ in the empty composer
// brings back the session's earlier prompts, which the fixture client serves without Rust.
// T37.24.5's: ⌘V of a PNG from a private pasteboard attaches it, and ⌘V of text is left to
// the Edit menu's Paste. T37.24.9's: `@` typed mid-text is completed in place, the caret after
// the insert. T37.24.7's: ⇧⇥ sends the mode the core's status names next.

import AppKit
import CoxClient
import CoxModel
import CoxTranscript
import CoxUI
import SwiftUI
import Testing

// Serialized: each test's window sends key events and runs the run loop, and a test waiting on a
// send lets another's window take the keys meanwhile.
@MainActor
@Suite(.serialized) struct ComposerFlowTests {
  @Test func typingAtPickingAFileAndSendingReachesTheClient() async throws {
    let rows = [
      Completion(insert: "@src/lib.rs", detail: "src/lib.rs"),
      Completion(insert: "@src/main.rs", detail: "src/main.rs"),
    ]
    let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []), completions: rows)
    let store = ComposerStore(session: SessionStore(session: session))
    let host = ComposerHost(SessionComposer(store: store))
    defer { host.close() }

    host.type("@")
    #expect(store.completions.map(\.insert) == rows.map(\.insert))
    host.press(.downArrow)
    host.press(.return)
    #expect(store.text == "@src/main.rs ")
    #expect(store.mentions == ["@src/main.rs"])

    host.type("explain it")
    host.press(.return)
    // The draft clears after the client took the intent, and the editor a run-loop turn later.
    await host.settle(until: { !session.sent.isEmpty && host.editor.string.isEmpty })
    #expect(session.sent == [.send(text: "@src/main.rs explain it", attachments: [])])
    #expect(host.editor.string.isEmpty)
  }

  @Test func aTokenTypedMidTextIsCompletedInPlaceAndTheCaretFollowsTheInsert() async throws {
    let rows = [Completion(insert: "@src/main.rs", detail: "src/main.rs")]
    let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []), completions: rows)
    let store = ComposerStore(session: SessionStore(session: session))
    let host = ComposerHost(SessionComposer(store: store))
    defer { host.close() }

    host.type("fix it")
    host.editor.setSelectedRange(NSRange(location: 3, length: 0))
    host.settle()
    host.type(" ")
    host.type("@")
    #expect(store.completions.map(\.insert) == ["@src/main.rs"])
    host.press(.return)
    #expect(host.editor.string == "fix @src/main.rs it")
    #expect(host.editor.selectedRange() == NSRange(location: 17, length: 0))

    host.type("and ")
    host.press(.return)
    await host.settle(until: { !session.sent.isEmpty })
    #expect(session.sent == [.send(text: "fix @src/main.rs and it", attachments: [])])
  }

  /// The editor keeps its own copy of the text, so the `!` the store turns into shell mode must
  /// leave it too.
  @Test func aBangTypedIntoTheEmptyComposerLeavesTheEditorEmptyInShellMode() {
    let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []))
    let store = ComposerStore(session: SessionStore(session: session))
    let host = ComposerHost(SessionComposer(store: store))
    defer { host.close() }

    host.type("!")
    #expect(store.isShell)
    #expect(host.editor.string.isEmpty)
    host.type("ls")
    #expect(store.text == "ls")
  }

  @Test func whileATurnRunsReturnQueuesAndCommandReturnInterruptsAndSends() async throws {
    let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []))
    let transcript = SessionStore(session: session)
    let tally = Tally(
      sent: 0, received: 0, cacheRead: 0, cacheWrite: 0, uncached: 0, costUsd: 0, calls: 0,
      estimated: false)
    let running = TurnUsage(
      turn: "t1", tally: tally, thinkingTokens: 0, ttftMs: nil, tokPerS: nil, exact: false,
      sparkline: [], done: false)
    transcript.apply([.usage(usage: UsageView(session: tally, turn: running, contextTokens: 0))])
    let store = ComposerStore(session: transcript)
    let host = ComposerHost(SessionComposer(store: store))
    defer { host.close() }

    host.type("next")
    host.press(.return)
    await host.settle(until: { !session.sent.isEmpty })
    #expect(session.sent == [.queue(text: "next", attachments: [])])

    host.type("now")
    host.press(.commandReturn)
    await host.settle(until: { session.sent.count == 3 })
    #expect(session.sent.suffix(2) == [.interrupt, .send(text: "now", attachments: [])])
  }

  @Test func upInTheEmptyComposerBringsBackTheEarlierPromptsNewestFirst() {
    let session = FixtureSession(
      fixture: Fixture(batches: [], snapshot: []), prompts: ["run the tests", "add a cache"])
    let store = ComposerStore(session: SessionStore(session: session))
    let host = ComposerHost(SessionComposer(store: store))
    defer { host.close() }

    host.press(.upArrow)
    #expect(store.text == "run the tests")
    #expect(host.editor.string == "run the tests")
    host.press(.upArrow)
    #expect(store.text == "add a cache")
    #expect(host.editor.string == "add a cache")
    host.press(.downArrow)
    #expect(host.editor.string == "run the tests")
  }

  @Test func shiftTabAsksForTheModeTheCoreNamesNext() async throws {
    let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []))
    let transcript = SessionStore(session: session)
    transcript.apply([.status(status: Status(mode: .plan, nextMode: .auto))])
    let store = ComposerStore(session: transcript)
    let host = ComposerHost(SessionComposer(store: store))
    defer { host.close() }

    host.press(.shiftTab)
    await host.settle(until: { !session.sent.isEmpty })
    #expect(session.sent == [.setMode(mode: .auto)])
    #expect(host.editor.string.isEmpty)
  }

  @Test func pastingAPNGAttachesItAndSendCarriesIt() async throws {
    let board = NSPasteboard(name: NSPasteboard.Name("cox.test.\(UUID())"))
    defer { board.releaseGlobally() }
    let rep = NSBitmapImageRep(
      bitmapDataPlanes: nil, pixelsWide: 2, pixelsHigh: 2, bitsPerSample: 8, samplesPerPixel: 4,
      hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0,
      bitsPerPixel: 0)
    let png = try #require(rep?.representation(using: .png, properties: [:]))
    board.clearContents()
    board.setData(png, forType: .png)
    let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []))
    let store = ComposerStore(session: SessionStore(session: session))
    let host = ComposerHost(SessionComposer(store: store).environment(\.composerPasteboard, board))
    defer { host.close() }

    #expect(!host.paste())
    #expect(store.attachments.count == 1)
    host.type("what is this")
    host.press(.return)
    await host.settle(until: { !session.sent.isEmpty })
    let image = Attachment(
      name: "Pasted image.png", mediaType: "image/png", dataB64: png.base64EncodedString())
    #expect(session.sent == [.send(text: "what is this", attachments: [image])])
  }

  @Test func pastingTextAttachesNothingAndLeavesTheKeyToTheMenu() throws {
    let board = NSPasteboard(name: NSPasteboard.Name("cox.test.\(UUID())"))
    defer { board.releaseGlobally() }
    board.clearContents()
    board.setString("plain words", forType: .string)
    let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []))
    let store = ComposerStore(session: SessionStore(session: session))
    let host = ComposerHost(SessionComposer(store: store).environment(\.composerPasteboard, board))
    defer { host.close() }

    #expect(host.paste())
    #expect(store.attachments.isEmpty)
  }
}

/// A view in a borderless window far off screen, with its text view first responder, so key
/// events travel the path a keyboard's do.
@MainActor
private final class ComposerHost {
  let window: NSWindow
  let editor: NSTextView

  enum Key {
    case upArrow, downArrow, `return`, commandReturn, shiftTab

    var code: UInt16 {
      switch self {
      case .upArrow: 126
      case .downArrow: 125
      case .return, .commandReturn: 36
      case .shiftTab: 48
      }
    }
    /// What a keyboard sends; ⇧⇥ arrives as the back-tab character.
    var characters: String {
      switch self {
      case .upArrow: "\u{F700}"
      case .downArrow: "\u{F701}"
      case .return, .commandReturn: "\r"
      case .shiftTab: "\u{19}"
      }
    }
    var modifiers: NSEvent.ModifierFlags {
      switch self {
      case .upArrow, .downArrow: [.numericPad, .function]
      case .return: []
      case .commandReturn: .command
      case .shiftTab: .shift
      }
    }
  }

  init(_ view: some View) {
    NSApplication.shared.setActivationPolicy(.accessory)
    // Room above the composer for the rows it floats there.
    let hosting = NSHostingView(
      rootView: view.frame(width: Size.readingWidth).padding(Size.toolbarHeight * 4))
    window = NSWindow(
      // A window frame far off screen, not a design size.
      // swiftlint:disable:next no_literal_size
      contentRect: NSRect(x: -20_000, y: -20_000, width: 1_100, height: 600),
      styleMask: [.borderless], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = hosting
    window.orderFrontRegardless()
    window.layoutIfNeeded()
    editor = Self.textViews(in: hosting).first ?? NSTextView()
    window.makeFirstResponder(editor)
    settle()
  }

  func type(_ text: String) {
    editor.insertText(text, replacementRange: editor.selectedRange())
    settle()
  }

  func press(_ key: Key) {
    let event = NSEvent.keyEvent(
      with: .keyDown, location: .zero, modifierFlags: key.modifiers,
      timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
      context: nil, characters: key.characters, charactersIgnoringModifiers: key.characters,
      isARepeat: false, keyCode: key.code)
    if let event { window.sendEvent(event) }
    settle()
  }

  /// ⌘V through the application, whose event monitors see a keyboard's ⌘V before the Edit menu
  /// does; says whether the key went on to a Paste item standing in for the menu's.
  func paste() -> Bool {
    let menu = PasteMenu()
    let previous = NSApp.mainMenu
    NSApp.mainMenu = menu.bar
    defer { NSApp.mainMenu = previous }
    let event = NSEvent.keyEvent(
      with: .keyDown, location: .zero, modifierFlags: .command,
      timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
      context: nil, characters: "v", charactersIgnoringModifiers: "v", isARepeat: false,
      keyCode: 9)
    if let event { NSApp.sendEvent(event) }
    settle()
    return menu.pasted
  }

  /// A few turns of the run loop, so SwiftUI applies what the store changed.
  func settle() {
    for _ in 0..<5 {
      window.layoutIfNeeded()
      RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.02))
    }
  }

  /// Yields to the tasks the view started — a send runs in one — until `done` holds or `limit`
  /// passes, then lets SwiftUI apply what they changed.
  func settle(until done: () -> Bool, limit: Duration = .seconds(5)) async {
    let deadline = ContinuousClock.now + limit
    while !done(), ContinuousClock.now < deadline {
      try? await Task.sleep(for: .milliseconds(20))
      settle()
    }
    settle()
  }

  func close() {
    window.orderOut(nil)
    window.close()
  }

  /// A menu bar whose one item takes ⌘V and records that it did.
  private final class PasteMenu: NSObject {
    let bar = NSMenu()
    private(set) var pasted = false

    override init() {
      super.init()
      let item = NSMenuItem(title: "Edit", action: nil, keyEquivalent: "")
      item.submenu = NSMenu(title: "Edit")
      let paste = NSMenuItem(title: "Paste", action: #selector(record), keyEquivalent: "v")
      paste.target = self
      item.submenu?.addItem(paste)
      bar.addItem(item)
    }

    @objc private func record() { pasted = true }
  }

  private static func textViews(in view: NSView) -> [NSTextView] {
    var found: [NSTextView] = []
    var stack: [NSView] = [view]
    while let next = stack.popLast() {
      if let match = next as? NSTextView { found.append(match) }
      stack.append(contentsOf: next.subviews)
    }
    return found
  }
}
