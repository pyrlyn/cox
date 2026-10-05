// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The menu-bar extra's rows (T51.14): answerable approvals and questions under their session's
// title, news and expired items left out, and the running rows as the sidebar has them.

import CoxClient
import Testing

@testable import CoxModel

@Test func theMenuListsOnlyWhatCanStillBeAnswered() {
  let approval = InboxItem(
    session: "s1", source: nil,
    need: .approval(call: "c1", tool: "bash", subject: "git push", why: .risk(risk: .exec)),
    expired: false, seq: 1, title: "bash git push", subtitle: "approval waiting", status: .waiting)
  let question = InboxItem(
    session: "s2", source: nil, need: .question(call: "c2", question: "Retry?", options: []),
    expired: false, seq: 2, title: "Retry?", subtitle: "question waiting", status: .waiting)
  let expired = InboxItem(
    session: "s3", source: nil, need: .question(call: "c3", question: "Old?", options: []),
    expired: true, seq: 3, title: "Old?", subtitle: "expired", status: .idle)
  let news = InboxItem(
    session: "s1", source: nil, need: .failed(text: "boom"), expired: false, seq: 4,
    title: "boom", subtitle: "turn failed", status: .error)
  let state = MenuBarState(
    inbox: [approval, question, expired, news], running: [], today: "$1.00 · 2 sessions"
  ) { $0 == "s1" ? "Migrate auth" : nil }
  #expect(
    state.needs == [
      .init(
        id: "s1#1", session: "s1", call: "c1", title: "Migrate auth",
        kind: .approval(command: "git push")),
      .init(
        id: "s2#2", session: "s2", call: "c2", title: "Untitled session",
        kind: .question("Retry?")),
    ])
  #expect(state.today == "$1.00 · 2 sessions")
}

@Test func runningRowsKeepTheSidebarsLine() {
  let row = SidebarRow(
    id: "s9", session: "s9", status: .running, title: "Add jitter", subtitle: "cox · running",
    cost: "$0.42", isReadOnly: false)
  let state = MenuBarState(inbox: [], running: [row], today: "") { _ in nil }
  #expect(
    state.running == [
      .init(id: "s9", title: "Add jitter", activity: "cox · running", cost: "$0.42")
    ]
  )
}
