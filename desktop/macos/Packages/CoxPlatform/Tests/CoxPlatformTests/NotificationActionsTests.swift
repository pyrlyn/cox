// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Notification actions without posting one (T37.27, T37.44.15): a note's content carries what
// its action needs, each action becomes its intent or the session it brings forward, and
// approving the recorded `approve-write` fixture's note from the notification resumes the
// waiting turn. The menu bar's Allow and Deny take the same route (T51.14).

import CoxClient
import Foundation
import Synchronization
import Testing
import UserNotifications

@testable import CoxPlatform

final class NoteHost: PlatformHost {
  let notes = Mutex<[HostNote]>([])

  func secret(for section: String) -> String? { nil }
  func notify(_ note: HostNote) { notes.withLock { $0.append(note) } }
  func badge(_ count: Int) {}
  func open(_ url: String) {}
}

/// `note`'s content answered with `action`, as the notification centre hands it back.
private func routed(_ note: HostNote, _ action: String, text: String? = nil) -> NotificationRoute? {
  NotificationActions.route(
    action: action, userInfo: NotificationActions.content(for: note).userInfo, text: text)
}

@Test func anApprovalOffersAllowOnceDenyAndOpenAndEachMapsToItsIntent() {
  let note = HostNote(session: "s1", kind: .approval, text: "bash git push", badge: 1, call: "c1")
  let content = NotificationActions.content(for: note)
  #expect(content.categoryIdentifier == NotificationActions.approval)
  #expect(content.threadIdentifier == "s1")
  #expect(content.title == "Approval needed")
  #expect(content.body == "bash git push")
  #expect(
    routed(note, NotificationActions.allow)
      == NotificationRoute(session: "s1", intent: .approve(call: "c1", decision: .allow)))
  #expect(
    routed(note, NotificationActions.deny)
      == NotificationRoute(session: "s1", intent: .approve(call: "c1", decision: .deniedByUser)))
  // Open answers nothing: it brings the session forward, as a click on the notification does.
  #expect(routed(note, NotificationActions.open) == nil)
  let userInfo = NotificationActions.content(for: note).userInfo
  #expect(NotificationActions.shows(action: NotificationActions.open, userInfo: userInfo) == "s1")
  #expect(
    NotificationActions.shows(action: UNNotificationDefaultActionIdentifier, userInfo: userInfo)
      == "s1")
  #expect(NotificationActions.shows(action: NotificationActions.allow, userInfo: userInfo) == nil)
  #expect(
    NotificationActions.shows(action: UNNotificationDismissActionIdentifier, userInfo: userInfo)
      == nil)
}

@Test func allowOnceNeverAllowsForTheSession() {
  let note = HostNote(session: "s1", kind: .approval, text: "bash git push", badge: 1, call: "c1")
  #expect(
    routed(note, NotificationActions.allow)?.intent
      != .approve(call: "c1", decision: .allowForSession))
}

@Test func allowFromTheMenuBarSendsTheNotificationPathsIntent() {
  let note = HostNote(session: "s1", kind: .approval, text: "bash git push", badge: 1, call: "c1")
  #expect(
    NotificationActions.approval(session: "s1", call: "c1", allow: true)
      == routed(note, NotificationActions.allow))
  #expect(
    NotificationActions.approval(session: "s1", call: "c1", allow: false)
      == routed(note, NotificationActions.deny))
  #expect(
    NotificationActions.approval(session: "s1", call: "c1", allow: true)?.intent
      == .approve(call: "c1", decision: .allow))
}

@Test func aQuestionTakesATypedAnswerAndIgnoresABlankOne() {
  let note = HostNote(session: "s1", kind: .question, text: "Which branch?", badge: 1, call: "q1")
  #expect(NotificationActions.content(for: note).categoryIdentifier == NotificationActions.question)
  #expect(
    routed(note, NotificationActions.answer, text: "  main\n")
      == NotificationRoute(session: "s1", intent: .answer(question: "q1", text: "main")))
  #expect(routed(note, NotificationActions.answer, text: " \n") == nil)
  #expect(routed(note, NotificationActions.answer) == nil)
}

@Test func newsHasNoActionsAndAPlainClickSendsNothing() {
  let news = HostNote(session: "s1", kind: .taskDone(succeeded: true), text: "tests", badge: 0)
  #expect(NotificationActions.content(for: news).categoryIdentifier.isEmpty)
  #expect(routed(news, NotificationActions.allow) == nil)
  let note = HostNote(session: "s1", kind: .approval, text: "write a.md", badge: 1, call: "c1")
  #expect(routed(note, UNNotificationDefaultActionIdentifier) == nil)
  #expect(routed(note, UNNotificationDismissActionIdentifier) == nil)
}

@Test func theCategoriesCarryTheActionsContentNames() {
  let byID = Dictionary(
    uniqueKeysWithValues: NotificationActions.categories.map { ($0.identifier, $0) })
  let approval = byID[NotificationActions.approval]?.actions
  #expect(
    approval?.map(\.identifier)
      == [NotificationActions.allow, NotificationActions.deny, NotificationActions.open])
  // Mockup 23's buttons; Open alone brings the app forward.
  #expect(approval?.map(\.title) == ["Allow once", "Deny", "Open"])
  #expect(approval?.map { $0.options.contains(.foreground) } == [false, false, true])
  let answer = byID[NotificationActions.question]?.actions.first
  #expect(answer?.identifier == NotificationActions.answer)
  #expect(answer is UNTextInputNotificationAction)
}

/// The recorder's `approve-write.json`, found from this file.
private let approveWrite = URL(filePath: #filePath)
  .deletingLastPathComponent()  // CoxPlatformTests
  .deletingLastPathComponent()  // Tests
  .deletingLastPathComponent()  // CoxPlatform
  .deletingLastPathComponent()  // Packages
  .deletingLastPathComponent()  // macos
  .appending(path: "Fixtures/approve-write.json")

@Test func allowingFromTheNotificationResumesTheRecordedTurn() async throws {
  let fixture = try Fixture(contentsOf: approveWrite)
  let host = NoteHost()
  let session = FixtureSession(fixture: fixture, host: host, waitsForYou: true)

  var pulled: [[TimelinePatch]] = []
  while host.notes.withLock({ $0.isEmpty }), let batch = await session.nextPatches() {
    pulled.append(batch)
  }
  let note = try #require(host.notes.withLock { $0 }.first)
  #expect(note.kind == .approval)
  #expect(note.badge == 1)

  let route = try #require(routed(note, NotificationActions.allow))
  #expect(route.session == note.session)
  let call = try #require(note.call)
  #expect(route.intent == .approve(call: call, decision: .allow))

  // Waiting on the approval, the rest of the turn arrives once the notification's intent does.
  let rest = Task { () -> [[TimelinePatch]] in
    var batches: [[TimelinePatch]] = []
    while let batch = await session.nextPatches() { batches.append(batch) }
    return batches
  }
  _ = try await session.send(route.intent)
  pulled += await rest.value

  #expect(pulled == fixture.batches)
  #expect(session.sent == [route.intent])
}
