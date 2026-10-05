// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The generated cox-ffi values convert to the `CoxClient` values a recorded
// fixture decodes to, so the stores see the same timeline live and replayed.

import CoxClient
import CoxFFIBindings
import Foundation
import Testing

@testable import CoxCore

@Test func aToolUpsertConvertsFieldForField() {
  let live = CoxFFIBindings.TimelinePatch.upsert(
    block: .init(
      id: "call:1", turn: 2,
      kind: .tool(
        tool: "read", summary: "Read `a`", icon: .read, risk: .readOnly, state: .done,
        tail: "1\thello", archive: .init(id: "01A", bytes: 24), diff: nil, durationMs: 7,
        pluginView: nil)),
    after: "item:1")
  let want = CoxClient.TimelinePatch.upsert(
    block: .init(
      id: "call:1", turn: 2,
      kind: .tool(
        tool: "read", summary: "Read `a`", icon: .read, risk: .readOnly, state: .done,
        tail: "1\thello", archive: .init(id: "01A", bytes: 24), diff: nil, durationMs: 7)),
    after: "item:1")
  #expect(CoxClient.TimelinePatch(live) == want)
}

@Test func anAssistantPluginViewConverts() {
  let span = CoxFFIBindings.SpanView(text: "hi", style: .ok, bold: false, italic: false)
  let live = CoxFFIBindings.BlockKind.assistant(
    text: "x", doc: .init(blocks: []), pluginView: .text(lines: [[span]]))
  #expect(
    CoxClient.BlockKind(live)
      == .assistant(
        text: "x", doc: .init(blocks: []),
        pluginView: .text(lines: [[PluginRun("hi", token: .ok)]])))
}

@Test func aDocTailKeepsSpanStyleColourAndLineStructure() {
  let span = CoxFFIBindings.Span(
    text: "fn", token: .accent, rgb: 0xB48EAD, light: 0x4F5B66, bold: true, italic: false,
    strike: false, underline: false, link: nil)
  let live = CoxFFIBindings.TimelinePatch.docTail(
    id: "item:2", from: 1,
    blocks: [
      .code(lang: "rust", lines: [[span]]), .text(kind: .heading(2), lines: []),
      .text(kind: .list, lines: [.init(quote: 1, depth: 2, marker: "3.", spans: [span])]),
    ])
  var want = CoxClient.Span(text: "fn")
  (want.token, want.rgb, want.light, want.bold) = (.accent, 0xB48EAD, 0x4F5B66, true)
  let blocks: [CoxClient.DocBlock] = [
    .code(lang: "rust", lines: [[want]]), .text(kind: .heading(2), lines: []),
    .text(kind: .list, lines: [CoxClient.TextLine([want], quote: 1, depth: 2, marker: "3.")]),
  ]
  #expect(CoxClient.TimelinePatch(live) == .docTail(id: "item:2", from: 1, blocks: blocks))
}

@Test func theCoreWritesADocAsMarkdownThroughTheSeam() {
  var bold = CoxClient.Span(text: "Plan")
  bold.bold = true
  let doc = CoxClient.StyledDoc(blocks: [
    .text(kind: .heading(2), lines: [CoxClient.TextLine([bold])]),
    .text(
      kind: .list,
      lines: [
        CoxClient.TextLine([.init(text: "one")], marker: "•"),
        CoxClient.TextLine([.init(text: "two")], depth: 1, marker: "•"),
      ]),
    .code(lang: "sh", lines: [[.init(text: "echo ```")]]),
    .table(rows: []),
  ])
  let writer = CoreDocWriter()
  #expect(writer.markdown(doc) == "## Plan\n\n- one\n  - two\n\n````sh\necho ```\n````")
  #expect(writer.markdown(.table(rows: [])) == nil)
  #expect(writer.markdown(.rule) == "---")
}

@Test func anApprovalIntentConvertsToTheGeneratedIntent() {
  let intent = CoxClient.Intent.approve(call: "c1", decision: .edit(input: #"{"path":"a"}"#))
  let want = CoxFFIBindings.Intent.approve(call: "c1", decision: .edit(input: #"{"path":"a"}"#))
  #expect(CoxFFIBindings.Intent(intent) == want)
  #expect(CoxFFIBindings.Intent(.setEffort(effort: nil)) == .setEffort(effort: nil))
}

@Test func aQueuedIntentKeepsItsAttachmentsAndThinkAndTheStatusKeepsEveryField() {
  let shot = CoxClient.Attachment(name: "shot.png", mediaType: "image/png", dataB64: "iVBO")
  let want = CoxFFIBindings.Intent.queue(
    text: "look", attachments: [.init(name: "shot.png", mediaType: "image/png", dataB64: "iVBO")],
    confirmThink: true)
  let queue = CoxClient.Intent.queue(text: "look", attachments: [shot], confirmThink: true)
  #expect(CoxFFIBindings.Intent(queue) == want)
  let live = CoxFFIBindings.TimelinePatch.status(
    status: .init(
      queued: 2, mode: .plan, nextMode: .auto, model: "claude-sonnet-5",
      modelName: "Claude Sonnet 5", shortName: "Sonnet 5", effort: .high))
  let status = CoxClient.Status(
    queued: 2, mode: .plan, nextMode: .auto, model: "claude-sonnet-5", effort: .high,
    modelName: "Claude Sonnet 5", shortName: "Sonnet 5")
  #expect(CoxClient.TimelinePatch(live) == .status(status: status))
}

@Test func aChangesRecordConvertsFieldForField() {
  let live = CoxFFIBindings.Changes(
    files: [.init(path: "a.rs", change: .created, added: 3, removed: 0, call: "c1", turn: 2)],
    checkpoints: [.init(turn: 2, label: "Turn 2 · before a.rs", time: "2026-09-28T14:02:00.000Z")],
    worktree: .init(path: "/w", branch: "t1", base: "main", commit: "4273daa", bytes: 9),
    worktreeFacts: [.init(label: "Branch", value: "t1", detail: false)],
    turns: [
      .init(
        turn: 2,
        files: [.init(path: "a.rs", change: .created, added: 3, removed: 0, call: "c1", turn: 2)])
    ])
  let want = CoxClient.Changes(
    files: [.init(path: "a.rs", change: .created, added: 3, removed: 0, call: "c1", turn: 2)],
    checkpoints: [.init(turn: 2, label: "Turn 2 · before a.rs", time: "2026-09-28T14:02:00.000Z")],
    worktree: .init(path: "/w", branch: "t1", base: "main", commit: "4273daa", bytes: 9),
    worktreeFacts: [.init(label: "Branch", value: "t1")],
    turns: [
      .init(
        turn: 2,
        files: [.init(path: "a.rs", change: .created, added: 3, removed: 0, call: "c1", turn: 2)])
    ])
  #expect(CoxClient.Changes(live) == want)
}

@Test func anInfoRecordCarriesItsFacts() {
  let live = CoxFFIBindings.Info(
    session: "s", cwd: "/w", worktree: nil, config: [], rollout: "/r.jsonl",
    facts: [.init(label: "Branch", value: "detached", detail: true)],
    configFacts: [.init(label: "~/.cox/config.toml", value: nil, detail: true)])
  let info = CoxClient.Info(live)
  #expect(info.facts == [CoxClient.Fact(label: "Branch", value: "detached", detail: true)])
  #expect(info.configFacts == [CoxClient.Fact(label: "~/.cox/config.toml", detail: true)])
}

@Test func aTodoItemConvertsWithEachState() {
  let live: [CoxFFIBindings.TodoItem] = [
    .init(id: "1", text: "Read", state: .done), .init(id: "2", text: "Test", state: .inProgress),
    .init(id: "3", text: "Push", state: .pending),
  ]
  let want: [CoxClient.TodoItem] = [
    .init(id: "1", text: "Read", state: .done), .init(id: "2", text: "Test", state: .inProgress),
    .init(id: "3", text: "Push", state: .pending),
  ]
  #expect(live.map { CoxClient.TodoItem($0) } == want)
}

@Test func turnCostsConvertFieldForField() {
  let row = CoxFFIBindings.CostRow(label: "explore", values: ["9.8k", "0.03"], detail: true)
  let total = CoxFFIBindings.CostRow(label: "Session", values: ["9.8k", "0.03"], detail: false)
  let live = CoxFFIBindings.TurnCosts(
    columns: ["In", "$"], rows: [row], total: total, project: "Project cox today: $0.03",
    budget: [CoxFFIBindings.BudgetRow(label: "Session", text: "$0.03 of $5.00", fraction: 0.006)])
  #expect(
    CoxClient.TurnCosts(live)
      == CoxClient.TurnCosts(
        columns: ["In", "$"],
        rows: [CoxClient.CostRow(label: "explore", values: ["9.8k", "0.03"], detail: true)],
        total: CoxClient.CostRow(label: "Session", values: ["9.8k", "0.03"]),
        project: "Project cox today: $0.03",
        budget: [CoxClient.BudgetRow(label: "Session", text: "$0.03 of $5.00", fraction: 0.006)]))
}

/// T37.44.13: a remote session's palette goes through the core's ranking and back: actions before
/// sessions, with the matched characters.
@Test func aRemoteSessionsPaletteIsRankedByTheCore() {
  let items = [
    CoxClient.PaletteItem(kind: .session, id: "s1", title: "Revert sitemap", detail: "acme"),
    CoxClient.PaletteItem(kind: .action, id: "review", title: "Review changes"),
    CoxClient.PaletteItem(kind: .action, id: "new", title: "New session"),
  ]
  let hits = CoxClient.PaletteHit.ranked("rev", items, limit: 5)
  #expect(hits.map(\.item.id) == ["review", "s1"])
  #expect(hits.map(\.matched) == [[0, 1, 2], [0, 1, 2]])
  #expect(hits.last?.item == items[0])
}

/// T58.4.12: the core's popover sections reach Swift in its order, the code tier first, with
/// each model once.
@Test func theModelMenuConvertsTheCoresSections() throws {
  let (home, client) = try scratch()
  defer { try? FileManager.default.removeItem(at: home) }
  let sections = try client.modelMenu(cwd: home.path())
  #expect(sections.first?.tier == .code)
  #expect(sections.first?.title == "Code")
  let ids = sections.flatMap { $0.models.map(\.id) }
  #expect(!ids.isEmpty)
  #expect(Set(ids).count == ids.count)
}

/// T58.4.24: a task block's state crosses as the core decided it.
@Test func aTaskStateConvertsCaseForCase() {
  let live: [CoxFFIBindings.TaskState] = [.running, .succeeded, .failed]
  #expect(live.map { CoxClient.TaskState($0) } == [.running, .succeeded, .failed])
}

/// T58.4.5: the core's sidebar rows keep their parts; the store localizes `age`, not this layer.
@Test func theCoresSidebarSectionsConvertFieldForField() {
  let live = CoxFFIBindings.SidebarSection(
    id: "running", title: "Running", kind: .section(count: nil),
    rows: [
      .init(
        id: "jitter", session: "jitter", status: .running, title: "Add retry jitter",
        subtitle: [
          .text(text: "cox"), .text(text: "running"),
          .age(updatedAt: "2026-09-29T12:00:00Z"),
        ], cost: 0.42, isReadOnly: true)
    ])
  let want = CoxClient.SidebarSection(
    id: "running", title: "Running", kind: .section(count: nil),
    rows: [
      .init(
        id: "jitter", session: "jitter", status: .running, title: "Add retry jitter",
        subtitle: [.text("cox"), .text("running"), .age(updatedAt: "2026-09-29T12:00:00Z")],
        cost: 0.42, isReadOnly: true)
    ])
  #expect(CoxClient.SidebarSection(live) == want)
  let kinds: [CoxFFIBindings.SidebarKind] = [.section(count: "2"), .project(isExpanded: false)]
  #expect(
    kinds.map { CoxClient.SidebarKind($0) } == [
      .section(count: "2"), .project(isExpanded: false),
    ])
  let statuses: [CoxFFIBindings.SidebarStatus] = [.running, .waiting, .idle, .error]
  #expect(
    statuses.map { CoxClient.SidebarStatus($0) } == [.running, .waiting, .idle, .error])
}

@Test func aScratchWorkspaceAnswersAnEmptySidebar() throws {
  let (home, client) = try scratch()
  defer { try? FileManager.default.removeItem(at: home) }
  #expect(try client.sidebar(filter: "", folded: []).isEmpty)
}
