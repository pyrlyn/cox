// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// HostBridge forwards Rust's host calls to a `PlatformHost` (DT§4.4): the
// secret it answers, the URL, and an inbox item as a `HostNote`. The
// platform host here records calls; it holds no Keychain (A49).

import CoxClient
import CoxFFIBindings
import Synchronization
import Testing

@testable import CoxCore

final class RecordingHost: PlatformHost {
  let notes = Mutex<[HostNote]>([])
  let opened = Mutex<[String]>([])
  let badges = Mutex<[Int]>([])

  func secret(for section: String) -> String? { section == "anthropic" ? "sk-ant" : nil }
  func notify(_ note: HostNote) { notes.withLock { $0.append(note) } }
  func badge(_ count: Int) { badges.withLock { $0.append(count) } }
  func open(_ url: String) { opened.withLock { $0.append(url) } }
}

@Test func theBridgeAnswersThePlatformHostsSecret() {
  let bridge = HostBridge(RecordingHost())
  #expect(bridge.secret(section: "anthropic") == "sk-ant")
  #expect(bridge.secret(section: "openai") == nil)
}

@Test func anInboxItemBecomesANoteForItsSession() {
  let host = RecordingHost()
  let bridge = HostBridge(host)
  let question = CoxFFIBindings.Need.question(callId: "c1", question: "Which branch?", options: [])
  bridge.notify(
    item: CoxFFIBindings.InboxItem(
      session: "s1", source: nil, need: question, expired: false, seq: 1, title: "Which branch?",
      subtitle: "question waiting", status: .waiting), badge: 2)
  let done = CoxFFIBindings.Need.taskDone(task: "t1", label: "tests", ok: false)
  bridge.notify(
    item: CoxFFIBindings.InboxItem(
      session: "s2", source: nil, need: done, expired: false, seq: 2, title: "tests",
      subtitle: "task failed", status: .error), badge: 0)
  bridge.openUrl(url: "https://example.com")
  bridge.badge(badge: 0)
  #expect(
    host.notes.withLock { $0 } == [
      HostNote(session: "s1", kind: .question, text: "Which branch?", badge: 2, call: "c1"),
      HostNote(session: "s2", kind: .taskDone(succeeded: false), text: "tests", badge: 0),
    ])
  #expect(host.opened.withLock { $0 } == ["https://example.com"])
  #expect(host.badges.withLock { $0 } == [0])
}

/// A browser pane showing one page; remembers what it was asked to load.
final class PageHost: PlatformHost {
  let loaded = Mutex<[String]>([])

  func secret(for section: String) -> String? { nil }
  func notify(_ note: HostNote) {}
  func badge(_ count: Int) {}
  func open(_ url: String) {}
  var hasBrowser: Bool { true }
  func browserLoad(_ url: String) async throws(CoxClient.BrowserFailure) {
    loaded.withLock { $0.append(url) }
  }
  func browserText() async throws(CoxClient.BrowserFailure) -> CoxClient.PageText {
    CoxClient.PageText(title: "Docs", url: "http://localhost:3000/", text: "hello")
  }
  func browserSnapshot() async throws(CoxClient.BrowserFailure) -> [UInt8] { throw .page("blank") }
}

@Test func theBridgeForwardsTheBrowserPane() async throws {
  let host = PageHost()
  let bridge = HostBridge(host)
  #expect(bridge.hasBrowser())
  try await bridge.browserLoad(url: "http://localhost:3000/")
  #expect(host.loaded.withLock { $0 } == ["http://localhost:3000/"])
  let page = try await bridge.browserText()
  #expect(page.title == "Docs" && page.url == "http://localhost:3000/" && page.text == "hello")
  await #expect(throws: CoxFFIBindings.BrowserFailure.self) {
    try await bridge.browserSnapshot()
  }
}

@Test func aHostWithoutAPaneOffersNoBrowser() async {
  let bridge = HostBridge(RecordingHost())
  #expect(!bridge.hasBrowser())
  await #expect(throws: CoxFFIBindings.BrowserFailure.self) {
    try await bridge.browserText()
  }
}
