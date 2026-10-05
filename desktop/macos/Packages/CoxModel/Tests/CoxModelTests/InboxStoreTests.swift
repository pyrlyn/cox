// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The "Needs you" store's check (T37.27.7): with the `approve-write` fixture it lists one row,
// which clears once the card is answered; and a row copies the core's words (T58.4.2; the words
// themselves are `cox_app::inbox`'s tests), an expired one read-only.

import CoxClient
import Foundation
import Testing

@testable import CoxModel

@MainActor
@Test func theRecordedApprovalIsOneRowThatClearsOnceAnswered() async throws {
  let fixture = try Fixture(contentsOf: try #require(approveWrite))
  let client = FixtureCoreClient(fixture: fixture, waitsForYou: true)
  let inbox = InboxStore(client: client)
  let session = try await client.open(OpenSession(cwd: "/", theme: "base16-ocean.dark"))
  let store = SessionStore(session: session)
  let run = Task { await store.run() }

  inbox.refresh()
  #expect(inbox.rows.isEmpty)
  let deadline = Date(timeIntervalSinceNow: 10)
  while pendingApproval(store) == nil, Date() < deadline { await Task.yield() }
  let call = try #require(pendingApproval(store))

  inbox.refresh()
  let item = try #require(fixture.notes.first?.item)
  #expect(
    inbox.rows == [
      InboxRow(
        id: "\(item.session)#1", session: item.session, status: .waiting,
        title: "write summary.md", subtitle: "approval waiting", isReadOnly: false)
    ])
  #expect(inbox.count == "1")

  _ = try await store.send(.approve(call: call, decision: .allow))
  await run.value
  inbox.refresh()
  #expect(inbox.rows.isEmpty)
  #expect(inbox.count == nil)
}

@Test func aRowCopiesTheCoresWordsAndAnExpiredOneIsReadOnly() {
  func item(expired: Bool) -> InboxItem {
    InboxItem(
      session: "s", source: Source(session: "child", agent: "reviewer", preset: nil),
      need: .question(call: "c2", question: "Which branch?", options: []), expired: expired,
      seq: 7, title: "core title", subtitle: "core subtitle", status: .error)
  }
  let expected = InboxRow(
    id: "s#7", session: "s", status: .error, title: "core title", subtitle: "core subtitle",
    isReadOnly: false)
  #expect(InboxRow(item(expired: false)) == expected)
  #expect(InboxRow(item(expired: true)).isReadOnly)
}
