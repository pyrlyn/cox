// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// SessionStore against the recorded fixtures (T37.16's Check) and the patch
// kinds the recorded scenarios do not reach yet.

import CoxClient
import Foundation
import Testing

@testable import CoxModel

/// Every `desktop/macos/Fixtures/*.json`, found from this file so the tests
/// read the recorder's output in place.
let fixtures: [URL] = {
  let dir = URL(filePath: #filePath)
    .deletingLastPathComponent()  // CoxModelTests
    .deletingLastPathComponent()  // Tests
    .deletingLastPathComponent()  // CoxModel
    .deletingLastPathComponent()  // Packages
    .deletingLastPathComponent()  // macos
    .appending(path: "Fixtures")
  let names = (try? FileManager.default.contentsOfDirectory(atPath: dir.path())) ?? []
  return names.filter { $0.hasSuffix(".json") }.sorted().map { dir.appending(path: $0) }
}()

@MainActor
@Test func fixturesAreFound() {
  #expect(!fixtures.isEmpty)
}

@MainActor
@Test(arguments: fixtures)
func replayingAFixtureEndsAtItsSnapshot(url: URL) async throws {
  let fixture = try Fixture(contentsOf: url)
  let client = FixtureCoreClient(fixture: fixture)
  let session = try await client.open(OpenSession(cwd: "/", theme: "base16-ocean.dark"))
  let store = SessionStore(session: session)

  await store.run()

  #expect(Array(store.blocks.values) == fixture.snapshot)
  #expect(Array(store.blocks.keys) == fixture.snapshot.map(\.id))
  #expect(store.usage != nil)
}

@MainActor
@Test func sendReachesTheSession() async throws {
  let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []))
  let store = SessionStore(session: session)

  _ = try await store.send(.send(text: "hi", attachments: []))

  #expect(session.sent == [.send(text: "hi", attachments: [])])
}

private func tool(_ id: BlockID, tail: String, pluginView: PluginView? = nil) -> Block {
  Block(
    id: id, turn: 1,
    kind: .tool(
      tool: "bash", summary: "Ran", icon: .shell, risk: .exec, state: .running, tail: tail,
      archive: nil, diff: nil, durationMs: 0, pluginView: pluginView))
}

private func paragraph(_ text: String) -> DocBlock {
  .text(kind: .paragraph, lines: [TextLine([Span(text: text)])])
}

@MainActor
@Test func upsertInsertsAfterItsAnchorAndReplacesInPlace() {
  let store = SessionStore(session: FixtureSession(fixture: Fixture(batches: [], snapshot: [])))
  store.apply([
    .upsert(block: Block(id: "a", turn: 1, kind: .thinking(text: "")), after: nil),
    .upsert(block: Block(id: "c", turn: 1, kind: .thinking(text: "")), after: "a"),
    .upsert(block: Block(id: "b", turn: 1, kind: .thinking(text: "")), after: "a"),
    .upsert(block: Block(id: "a", turn: 1, kind: .thinking(text: "x")), after: nil),
    .upsert(block: Block(id: "z", turn: 1, kind: .thinking(text: "")), after: "gone"),
  ])
  #expect(Array(store.blocks.keys) == ["a", "b", "c", "z"])
  #expect(store.blocks["a"]?.kind == .thinking(text: "x"))
}

@MainActor
@Test func appendTextKeepsTheLastFiveLinesOfAToolTail() {
  let store = SessionStore(session: FixtureSession(fixture: Fixture(batches: [], snapshot: [])))
  store.apply([
    .reset(blocks: [
      tool("t", tail: "1\n2\n3\n"), Block(id: "k", turn: 1, kind: .thinking(text: "a")),
    ]),
    .appendText(id: "t", text: "4\r\n5\n6\n"),
    .appendText(id: "k", text: "b"),
  ])
  #expect(store.blocks["t"] == tool("t", tail: "2\n3\n4\r\n5\n6\n"))
  #expect(store.blocks["k"]?.kind == .thinking(text: "ab"))
}

@MainActor
@Test func docTailReplacesFromItsIndexAndRemoveDrops() {
  let store = SessionStore(session: FixtureSession(fixture: Fixture(batches: [], snapshot: [])))
  let doc = StyledDoc(blocks: [paragraph("a"), paragraph("b")])
  store.apply([
    .reset(blocks: [
      Block(id: "m", turn: 1, kind: .assistant(text: "", doc: doc)), tool("t", tail: ""),
    ]),
    .docTail(id: "m", from: 1, blocks: [paragraph("B"), paragraph("c")]),
    .docTail(id: "m", from: 9, blocks: [paragraph("ignored")]),
    .remove(id: "t"),
  ])
  let want = StyledDoc(blocks: [paragraph("a"), paragraph("B"), paragraph("c")])
  #expect(store.blocks["m"]?.kind == .assistant(text: "", doc: want))
  #expect(Array(store.blocks.keys) == ["m"])
}

/// A reply streamed as `cox_app`'s timeline streams it: each chunk re-parsed, only the blocks
/// from the first changed one re-sent. It starts from a snapshot that already holds a source,
/// the case a stale source showed in; Markdown is the core's writer's job at Copy (T58.4.27).
@MainActor
@Test func aStreamedReplysTextIsEmptyAfterEveryDocTail() {
  let store = SessionStore(session: FixtureSession(fixture: Fixture(batches: [], snapshot: [])))
  let heading = DocBlock.text(kind: .heading(1), lines: [TextLine([Span(text: "Title")])])
  let firstPara = DocBlock.text(kind: .paragraph, lines: [TextLine([Span(text: "first para")])])
  let fullPara = DocBlock.text(kind: .paragraph, lines: [TextLine([Span(text: "first paragraph")])])
  let code = DocBlock.code(lang: "swift", lines: [[Span(text: "let x = 1")]])
  let stream: [TimelinePatch] = [
    .docTail(id: "m", from: 1, blocks: [firstPara]),
    .docTail(id: "m", from: 1, blocks: [fullPara]),
    .docTail(id: "m", from: 2, blocks: [code]),
  ]
  store.apply([
    .reset(blocks: [
      Block(id: "m", turn: 1, kind: .assistant(text: "# Title", doc: StyledDoc(blocks: [heading])))
    ])
  ])

  for patch in stream {
    store.apply([patch])
    guard case .assistant(let copied, _, _) = store.blocks["m"]?.kind else {
      Issue.record("the reply is gone after \(patch)")
      return
    }
    #expect(copied.isEmpty)
  }
}

@Test func lastLinesMatchesTheRustTail() {
  #expect(lastLines("a\nb") == "a\nb")
  #expect(lastLines("1\n2\n3\n4\n5\n6\n") == "2\n3\n4\n5\n6\n")
  #expect(lastLines("1\n2\n3\n4\n5\n6") == "2\n3\n4\n5\n6")
}

/// T58.4.24: a task block's state is read from the wire as the core decided it, never from its
/// exit code; no recorded scenario reaches a task yet, so the block is spelled out here.
@Test func aTaskBlockDecodesTheCoresState() throws {
  let json = """
    {"id": "task:#5", "turn": 1, "kind": {"type": "task", "task": "#5", "label": "explore: x",
    "tier": "cheap", "done": true, "cost_usd": 0.0, "exit_code": null, "state": "succeeded",
    "kind": "agent"}}
    """
  let block = try JSONDecoder().decode(Block.self, from: Data(json.utf8))
  #expect(
    block.kind
      == .task(
        task: "#5", label: "explore: x", tier: .cheap, done: true, costUsd: 0, exitCode: nil,
        state: .succeeded, kind: .agent))
}

/// T52.23.2: a tool block's plugin tree is on the block, so an upsert that replaces the
/// block replaces the tree, as a slot patch replaces only its slot.
@MainActor
@Test func aToolBlocksPluginViewReplacesOnUpsert() {
  let store = SessionStore(session: FixtureSession(fixture: Fixture(batches: [], snapshot: [])))
  let first = PluginView.text(lines: [[PluginRun("a")]])
  let second = PluginView.text(lines: [[PluginRun("b")]])
  store.apply([.upsert(block: tool("t", tail: "", pluginView: first), after: nil)])
  #expect(store.blocks["t"]?.kind.pluginView == first)
  store.apply([.upsert(block: tool("t", tail: "", pluginView: second), after: nil)])
  #expect(store.blocks["t"]?.kind.pluginView == second)
}
