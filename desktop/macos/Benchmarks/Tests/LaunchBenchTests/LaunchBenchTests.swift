// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The launch bench's rules, without an app: the expanded fixture is one the app decodes into
// distinct blocks, only a visible app window ends the launch, and the median decides a budget.

import Foundation
import LaunchBench
import Testing

private let recorded = """
  {"batches": [], "snapshot": [
    {"id": "turn:a", "turn": 1, "kind": {"type": "turn_meta"}},
    {"id": "item:b", "turn": 1, "kind": {"type": "user", "text": "hi"}},
    {"id": "item:c", "turn": 2, "kind": {"type": "assistant", "text": "ok"}}
  ]}
  """

private func window(pid: Int32, layer: Int = 0, alpha: Double = 1, width: Double = 800)
  -> [String: Any]
{
  [
    "kCGWindowOwnerPID": pid, "kCGWindowLayer": layer, "kCGWindowAlpha": alpha,
    "kCGWindowBounds": ["X": 0.0, "Y": 0.0, "Width": width, "Height": 600.0],
  ]
}

struct LaunchBenchTests {
  @Test func expandedFixtureIsOneResetOfDistinctBlocksInLaterTurns() throws {
    let data = try expand(fixture: Data(recorded.utf8), to: 7)
    let root = try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
    let batches = try #require(root["batches"] as? [[[String: Any]]])
    #expect(batches.count == 1 && batches[0].count == 1)
    #expect(batches[0][0]["op"] as? String == "reset")
    let blocks = try #require(batches[0][0]["blocks"] as? [[String: Any]])
    let ids = blocks.compactMap { $0["id"] as? String }
    #expect(ids.count == 7 && Set(ids).count == 7, "every copy has its own id: \(ids)")
    #expect(blocks.compactMap { $0["turn"] as? Int } == [1, 1, 2, 3, 3, 4, 5])
    #expect((root["snapshot"] as? [Any])?.count == 7)
  }

  @Test func fixtureWithoutSnapshotBlocksIsRefused() {
    #expect(throws: BenchError.self) {
      try expand(fixture: Data(#"{"batches": [], "snapshot": []}"#.utf8), to: 10)
    }
  }

  @Test func onlyAVisibleWindowOfTheAppEndsTheLaunch() {
    #expect(showsWindow([window(pid: 7)], pid: 7))
    #expect(!showsWindow([window(pid: 8)], pid: 7), "another app's window")
    #expect(!showsWindow([window(pid: 7, layer: 25)], pid: 7), "the menu-bar extra")
    #expect(!showsWindow([window(pid: 7, alpha: 0)], pid: 7), "a transparent window")
    #expect(!showsWindow([window(pid: 7, width: 0)], pid: 7), "a zero-size helper")
  }

  @Test func theMedianDecidesTheBudget() {
    let slowFirst = Samples("launch", unit: "ms", budget: 400, values: [900, 300, 350])
    #expect(slowFirst.median == 350)
    #expect(slowFirst.withinBudget, "one slow run is not a miss")
    let even = Samples("rss", unit: "MB", budget: 150, values: [140, 170, 160, 100])
    #expect(even.median == 150 && even.withinBudget)
    #expect(!Samples("rss", unit: "MB", budget: 150, values: [151]).withinBudget)
    #expect(!Samples("none", unit: "ms", budget: 1, values: []).withinBudget)
  }
}
