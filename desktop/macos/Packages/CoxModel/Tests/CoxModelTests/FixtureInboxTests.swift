// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The inbox through the fixture client (T37.27): a recorded approval reaches the host as its
// `HostNote` with badge 1 when its batch is pulled, the turn waits on the person, and the
// approve intent resumes it to the recorded end. Plus each `Need` as its note's kind, with the
// core's title as its text (T58.4.2).

import CoxClient
import Foundation
import Synchronization
import Testing

@testable import CoxModel

final class NoteHost: PlatformHost {
  let notes = Mutex<[HostNote]>([])

  func secret(for section: String) -> String? { nil }
  func notify(_ note: HostNote) { notes.withLock { $0.append(note) } }
  func badge(_ count: Int) {}
  func open(_ url: String) {}
}

let approveWrite = fixtures.first { $0.lastPathComponent == "approve-write.json" }

/// The first pending approval's call id, once the store holds one.
@MainActor
func pendingApproval(_ store: SessionStore) -> String? {
  store.blocks.values.lazy.compactMap { block -> String? in
    guard case .approval(let call, _, _, _, _, _, _, nil, _) = block.kind else { return nil }
    return call
  }.first
}

@MainActor
@Test func aRecordedApprovalIsNotedWithBadgeOneAndApprovingResumesTheTurn() async throws {
  let fixture = try Fixture(contentsOf: try #require(approveWrite))
  let host = NoteHost()
  let client = FixtureCoreClient(fixture: fixture, host: host, waitsForYou: true)
  let session = try await client.open(OpenSession(cwd: "/", theme: "base16-ocean.dark"))
  let store = SessionStore(session: session)
  let run = Task { await store.run() }

  let deadline = Date(timeIntervalSinceNow: 10)
  while pendingApproval(store) == nil, Date() < deadline { await Task.yield() }
  let call = try #require(pendingApproval(store))
  // Parked: nothing past the pending card arrives until the person answers.
  for _ in 0..<50 { await Task.yield() }
  #expect(pendingApproval(store) == call)
  #expect(store.blocks.values.last { if case .checkpoint = $0.kind { true } else { false } } == nil)
  let note = try #require(host.notes.withLock { $0 }.first)
  #expect(note.kind == .approval)
  #expect(note.badge == 1)
  #expect(note.call == call)
  #expect(note.text == "write summary.md")

  _ = try await store.send(.approve(call: call, decision: .allow))
  await run.value

  #expect(pendingApproval(store) == nil)
  #expect(Array(store.blocks.values) == fixture.snapshot)
  #expect(host.notes.withLock { $0.count } == 1)
}

@Test func eachNeedBecomesItsNoteKindWithTheCoresTitle() {
  let why = Why.risk(risk: .exec)
  func note(_ need: Need) -> HostNote {
    HostNote(
      InboxItem(
        session: "s", source: nil, need: need, expired: false, seq: 1, title: "core title",
        subtitle: "", status: .waiting), badge: 2)
  }
  func expected(_ kind: HostNote.Kind, _ call: String? = nil) -> HostNote {
    HostNote(session: "s", kind: kind, text: "core title", badge: 2, call: call)
  }
  #expect(
    note(.approval(call: "c1", tool: "bash", subject: "git push", why: why))
      == expected(.approval, "c1"))
  #expect(
    note(.question(call: "c3", question: "Which branch?", options: []))
      == expected(.question, "c3"))
  #expect(note(.failed(text: "boom")) == expected(.failed))
  #expect(
    note(.taskDone(task: "t", label: "tests", succeeded: false))
      == expected(.taskDone(succeeded: false)))
}
