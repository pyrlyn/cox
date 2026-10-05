// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The sidebar's session list (T58.4.5): sections from the workspace client, subtitle parts
// joined with ` · ` and only `age` localized; filter and folded forwarded to the core; a
// recording with no workspace still shows the inbox; the toolbar's title and project come
// from a session's entry; the list re-reads when the workspace changed; and the providers'
// footer counts the usable providers (A110) with the checklist's key health as its dot.

import CoxClient
import Foundation
import Testing

@testable import CoxModel

private let now = Date(timeIntervalSince1970: 1_790_000_000)

private func ago(_ seconds: TimeInterval) -> String {
  Date(timeInterval: -seconds, since: now).formatted(.iso8601)
}

private let workspace = FixtureWorkspace(
  projects: [
    Project(root: "/src/cox", name: "cox"), Project(root: "/src/acme-web", name: "acme-web"),
  ],
  sessions: [
    "/src/cox": [
      SessionEntry(
        id: "jitter", title: "Add retry jitter", cwd: "/src/cox", updatedAt: ago(60), turns: 3,
        costUsd: 0.42),
      SessionEntry(
        id: "bench", title: "Bench plugin cold start", cwd: "/src/cox", updatedAt: ago(7200),
        turns: 5, costUsd: 1.18),
    ],
    "/src/acme-web": [
      SessionEntry(id: "sitemap", title: "Sitemap generator", updatedAt: ago(3600), turns: 1),
      SessionEntry(id: "fresh", updatedAt: ago(30)),
    ],
  ],
  sidebar: [
    CoxClient.SidebarSection(
      id: "running", title: "Running", kind: .section(count: nil),
      rows: [
        CoxClient.SidebarRow(
          id: "jitter", session: "jitter", status: .running, title: "Add retry jitter",
          subtitle: [.text("cox"), .text("running")], cost: 0.42)
      ]),
    CoxClient.SidebarSection(
      id: "/src/cox", title: "cox", kind: .project(isExpanded: true),
      rows: [
        CoxClient.SidebarRow(
          id: "bench", session: "bench", status: .idle, title: "Bench plugin cold start",
          subtitle: [.age(updatedAt: ago(7200)), .text("done")], cost: 1.18)
      ]),
    CoxClient.SidebarSection(
      id: "/src/acme-web", title: "acme-web", kind: .project(isExpanded: true),
      rows: [
        CoxClient.SidebarRow(
          id: "sitemap", session: "sitemap", status: .error, title: "Sitemap generator",
          subtitle: [.age(updatedAt: ago(3600)), .text("failed")]),
        CoxClient.SidebarRow(
          id: "fresh", session: "fresh", status: .idle, title: SessionEntry.untitled,
          subtitle: [.age(updatedAt: ago(30))]),
      ]),
  ])

@MainActor
private func listed() -> SidebarStore {
  let store = SidebarStore(
    workspace: workspace, inbox: nil, locale: Locale(identifier: "en_US_POSIX"))
  store.refresh(now: now)
  return store
}

@MainActor
@Test func sectionsJoinSubtitlePartsAndLocalizeOnlyAge() {
  let sections = listed().sections
  #expect(sections.map(\.id) == ["running", "/src/cox", "/src/acme-web"])
  #expect(sections[0].rows[0].subtitle == "cox · running")
  #expect(sections[0].rows[0].cost == "$0.42")
  let stamp = ago(7200)
  #expect(sections[1].rows[0].subtitle == "2h ago · done")
  #expect(!sections[1].rows[0].subtitle.contains(stamp))
  #expect(sections[2].rows[0].subtitle == "1h ago · failed")
  #expect(sections[2].rows[1].title == "Untitled session")
  #expect(sections[2].rows[1].cost == nil)
}

@MainActor
@Test func filterAndFoldedAreForwardedToTheWorkspace() {
  let client = Recording()
  let store = SidebarStore(workspace: client, inbox: nil)
  store.refresh()
  store.toggle("/src/acme-web")
  store.filter = "sitemap"
  #expect(client.lastFilter == "sitemap")
  #expect(Set(client.lastFolded) == ["/src/acme-web"])
}

/// A recording has no workspace: the inbox is still the first section.
@MainActor
@Test func theInboxComesFirstWithItsCount() {
  let item = InboxItem(
    session: "s1", source: nil,
    need: .approval(call: "c1", tool: "write", subject: "a.rs", why: .risk(risk: .write)),
    expired: false, seq: 1, title: "write a.rs", subtitle: "approval waiting", status: .waiting)
  let store = SidebarStore(workspace: nil, inbox: InboxStore(client: FixedInbox(items: [item])))
  store.refresh()
  let needs = store.sections.first
  #expect(needs?.id == "needs-you")
  #expect(needs?.kind == .section(count: "1"))
  #expect(needs?.rows.map(\.status) == [.waiting])
  #expect(needs?.rows[0].subtitle == "approval waiting")
}

@MainActor
@Test func aSessionsEntryNamesTheToolbarsTitleAndProject() {
  let store = listed()
  let entry = store.entry("bench")
  #expect(entry?.session.title == "Bench plugin cold start")
  #expect(entry?.project.name == "cox")
  #expect(store.entry("nope") == nil)
}

@MainActor
@Test func paletteItemsUseTheCoresSessionName() {
  let items = listed().paletteItems
  #expect(items.map(\.id) == ["jitter", "bench", "sitemap", "fresh"])
  #expect(items[0].title == "Add retry jitter")
  #expect(items.last?.title == "Untitled session")
}

@Test func theFooterCountsUsableProvidersAndShowsTheKeyCheck() {
  let passed = CheckRow(id: .providerKey, status: .passed, detail: "anthropic key found")
  let health = ProviderHealth(usable: ["anthropic", "local", "openai"], check: passed)
  #expect(health.text == "3 providers")
  #expect(health.status == .running)
  #expect(ProviderHealth(usable: ["anthropic"], check: nil).text == "1 provider")
  let failed = CheckRow(id: .providerKey, status: .failed, detail: "")
  #expect(ProviderHealth(usable: [], check: failed).status == .error)
  #expect(ProviderHealth(usable: [], check: failed).text == "0 providers")
  #expect(ProviderHealth(usable: nil, check: failed).text.isEmpty)
}

/// A workspace whose sessions grow by one each time it reports a change.
private final class Growing: WorkspaceClient, @unchecked Sendable {
  private let lock = NSLock()
  private var count = 0
  private let project = Project(root: "/src/cox", name: "cox")

  func projects(limit: UInt32) -> [Project] { [project] }
  func sessions(project: String, limit: UInt32) -> [SessionEntry] {
    lock.withLock { (0..<count).map { SessionEntry(id: "s\($0)") } }
  }
  func activity(session: String) -> Activity { .idle }
  func sidebar(filter: String, folded: [String]) -> [CoxClient.SidebarSection] {
    let rows = sessions(project: project.root, limit: 20).map {
      CoxClient.SidebarRow(
        id: $0.id, session: $0.id, status: .idle, title: $0.name)
    }
    return [
      CoxClient.SidebarSection(
        id: project.root, title: project.name, kind: .project(isExpanded: true), rows: rows)
    ]
  }
  func changed() async throws {
    try await Task.sleep(for: .milliseconds(10))
    lock.withLock { count += 1 }
  }
  func rename(session: String, title: String) -> Bool { false }
}

/// One session whose title a rename sets, as `cox.db` keeps it.
private final class Titled: WorkspaceClient, @unchecked Sendable {
  private let lock = NSLock()
  private var title: String?
  private let project = Project(root: "/src/cox", name: "cox")

  func projects(limit: UInt32) -> [Project] { [project] }
  func sessions(project: String, limit: UInt32) -> [SessionEntry] {
    lock.withLock { [SessionEntry(id: "s1", title: title)] }
  }
  func activity(session: String) -> Activity { .idle }
  func sidebar(filter: String, folded: [String]) -> [CoxClient.SidebarSection] {
    let entry = sessions(project: project.root, limit: 1)[0]
    return [
      CoxClient.SidebarSection(
        id: project.root, title: project.name, kind: .project(isExpanded: true),
        rows: [
          CoxClient.SidebarRow(
            id: entry.id, session: entry.id, status: .idle, title: entry.name)
        ])
    ]
  }
  func changed() async throws { try await Task.sleep(for: .seconds(86_400)) }
  func rename(session: String, title: String) -> Bool {
    lock.withLock { self.title = title }
    return true
  }
}

/// An inbox the store can read without a recorded session.
private struct FixedInbox: InboxClient {
  let items: [InboxItem]
  func inbox() -> [InboxItem] { items }
}

/// Records the filter and folded roots the store sent, so a Swift-side re-filter is not needed.
private final class Recording: WorkspaceClient, @unchecked Sendable {
  var lastFilter = ""
  var lastFolded: [String] = []

  func projects(limit: UInt32) -> [Project] { [] }
  func sessions(project: String, limit: UInt32) -> [SessionEntry] { [] }
  func activity(session: String) -> Activity { .idle }
  func sidebar(filter: String, folded: [String]) -> [CoxClient.SidebarSection] {
    lastFilter = filter
    lastFolded = folded
    return []
  }
  func changed() async throws { try await Task.sleep(for: .seconds(86_400)) }
  func rename(session: String, title: String) -> Bool { false }
}

/// A113: a sidebar rename reads the list again, so the row and the toolbar show the new title.
@MainActor
@Test func aRenameShowsTheNewTitleInTheRowAndTheToolbar() {
  let store = SidebarStore(workspace: Titled(), inbox: nil)
  store.refresh()
  #expect(store.sections.first?.rows.first?.title == "Untitled session")
  store.rename("s1", to: "Fix the ledger")
  #expect(store.sections.first?.rows.first?.title == "Fix the ledger")
  #expect(store.entry("s1")?.session.name == "Fix the ledger")
}

@MainActor
@Test func theListReadsAgainEachTimeTheWorkspaceChanged() async {
  let store = SidebarStore(workspace: Growing(), inbox: nil)
  let watching = Task { await store.watch() }
  defer { watching.cancel() }
  let deadline = Date(timeIntervalSinceNow: 10)
  while (store.sections.first?.rows.count ?? 0) < 2, Date() < deadline {
    try? await Task.sleep(for: .milliseconds(5))
  }
  #expect((store.sections.first?.rows.count ?? 0) >= 2)
  #expect(store.sections.first?.rows.first?.title == "Untitled session")
}
