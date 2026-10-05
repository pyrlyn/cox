// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// T37.27.5's Check: with the recorded `approve-write` fixture waiting on its write, the
// `SessionComposer` pins the `DecisionBar` above the composer, in light and dark; ⌘⏎ through the
// application sends `.approve` for that call and ⌘⌫ denies it, and the bar goes once the core
// has decided. Also which blocks pin a bar and what it says.

import AppKit
import CoxClient
import CoxModel
import CoxUI
import SnapshotTesting
import SwiftUI
import Testing

@testable import CoxTranscript

/// `desktop/macos/Fixtures/approve-write.json`, found from this file so the test reads the
/// recorder's output in place.
private let approveWrite = URL(filePath: #filePath)
  .deletingLastPathComponent()  // CoxTranscriptTests
  .deletingLastPathComponent()  // Tests
  .deletingLastPathComponent()  // CoxTranscript
  .deletingLastPathComponent()  // Packages
  .deletingLastPathComponent()  // macos
  .appending(path: "Fixtures/approve-write.json")

/// The call the fixture's write waits on: its first undecided approval, read from the recording
/// so a re-record keeps the test valid.
private let writeCall: String = {
  guard let fixture = try? Fixture(contentsOf: approveWrite) else { return "" }
  for case .upsert(let block, _) in fixture.batches.joined() {
    if case .approval(let call, _, _, _, _, _, _, nil, _) = block.kind { return call }
  }
  return ""
}()

@Test func onlyAWaitingApprovalOrQuestionPinsABar() {
  func approval(_ tool: String, decision: Decision? = nil) -> Block {
    Block(
      id: "a", turn: 1,
      kind: .approval(
        call: "c1", tool: tool, summary: "git push", input: "{}", grants: [],
        why: .risk(risk: .exec), source: nil,
        decision: decision, by: nil))
  }
  #expect(approval("bash").waiting?.bar == .approval("git push"))
  #expect(approval("write").waiting?.bar == .approval("write git push"))
  #expect(approval("bash", decision: .allow).waiting == nil)
  let question = Block(
    id: "q", turn: 1,
    kind: .question(call: "c2", question: "Which?", options: ["a", "b"], answer: nil))
  #expect(question.waiting?.call == "c2")
  #expect(question.waiting?.bar == .question("Which?", options: ["a", "b"]))
  #expect(DecisionBar.Choice.answer("a").intent(call: "c2") == .answer(question: "c2", text: "a"))
  #expect(Block(id: "u", turn: 1, kind: .user(text: "hi", attachments: [])).waiting == nil)
}

// The reading column and the pane's inset around it; the height follows the view.
// swiftlint:disable:next no_literal_size
private let size = NSSize(width: 784, height: 200)

@MainActor
@Suite(.serialized)
struct PinnedDecisionTests {
  @Test(arguments: [false, true])
  func aWaitingApprovalIsPinnedAboveTheComposer(dark: Bool) async throws {
    let waiting = try await Waiting()
    defer { waiting.close() }
    let host = waiting.host(dark: dark)
    defer { host.close() }
    // Anti-aliasing differs slightly between machines; a real change moves far more pixels.
    assertSnapshot(
      of: try host.image(), as: .image(precision: 0.995, perceptualPrecision: 0.98),
      named: dark ? "dark-solid" : "light-solid",
      testName: "aWaitingApprovalIsPinnedAboveTheComposer")
  }

  @Test func commandReturnAllowsTheWaitingCallAndTheBarGoes() async throws {
    let waiting = try await Waiting()
    defer { waiting.close() }
    let host = waiting.host()
    defer { host.close() }

    press(in: host.window, "\r", keyCode: 36)
    try await until { !waiting.session.sent.isEmpty }
    #expect(waiting.session.sent == [.approve(call: writeCall, decision: .allow)])
    try await until { waiting.store.waiting == nil }
    #expect(waiting.store.waiting == nil)
  }

  @Test func commandDeleteDeniesTheWaitingCall() async throws {
    let waiting = try await Waiting()
    defer { waiting.close() }
    let host = waiting.host()
    defer { host.close() }

    press(in: host.window, "\u{7F}", keyCode: 51)
    try await until { !waiting.session.sent.isEmpty }
    #expect(waiting.session.sent == [.approve(call: writeCall, decision: .deniedByUser)])
  }

  /// ⌘ and a key through the application, whose event monitors see it before the window's
  /// first responder does, as a keyboard's key.
  private func press(in window: NSWindow, _ key: String, keyCode: UInt16) {
    let event = NSEvent.keyEvent(
      with: .keyDown, location: .zero, modifierFlags: .command,
      timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
      context: nil, characters: key, charactersIgnoringModifiers: key, isARepeat: false,
      keyCode: keyCode)
    if let event { NSApp.sendEvent(event) }
  }
}

/// Yields to the session's run until `done` holds or ten seconds pass.
@MainActor
private func until(_ done: () -> Bool) async throws {
  let deadline = ContinuousClock.now + .seconds(10)
  while !done(), ContinuousClock.now < deadline {
    try await Task.sleep(for: .milliseconds(10))
  }
}

/// The fixture's session played until its write waits on the person.
@MainActor
private struct Waiting {
  let session: FixtureSession
  let store: SessionStore

  init() async throws {
    session = FixtureSession(fixture: try Fixture(contentsOf: approveWrite), waitsForYou: true)
    let store = SessionStore(session: session)
    self.store = store
    Task { await store.run() }
    try await until { store.waiting != nil }
    #expect(store.waiting?.call == writeCall)
  }

  /// The session's composer on a Solid pane, laid out at its height.
  func host(dark: Bool = false) -> Host {
    let host = Host([], size: size, dark: dark)
    host.hosting.rootView = AnyView(
      SessionComposer(store: ComposerStore(session: store))
        .padding(Space.l)
        .environment(\.coxAppearance, Appearance(material: .solid)))
    host.window.setContentSize(NSSize(width: size.width, height: host.hosting.fittingSize.height))
    // No transcript text is left to settle; a few turns let SwiftUI place the views.
    RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.1))
    host.flush()
    return host
  }

  /// Ends the session, so a run parked on the approval returns.
  func close() { session.close() }
}
