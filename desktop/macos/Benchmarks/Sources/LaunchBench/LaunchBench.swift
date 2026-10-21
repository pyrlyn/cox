// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The rules `cox-bench` measures by (T37.33): how a recorded fixture grows to the 2 000 blocks
// DT§1 names, which window counts as the app's first, and how runs sum up against a budget.
// Separate from the executable so they are tested without launching an app.

import Foundation

public enum BenchError: Error, CustomStringConvertible {
  case noSnapshot

  public var description: String {
    switch self {
    case .noSnapshot: "the fixture has no snapshot blocks to repeat"
    }
  }
}

/// `fixture`'s final blocks repeated, each copy with fresh ids and later turns, until there are
/// `count`, as one `reset` batch. A recorded session's real block shapes at the size DT§1 names,
/// so no block JSON is written by hand and the shapes follow the recorder.
public func expand(fixture: Data, to count: Int) throws -> Data {
  guard let root = try JSONSerialization.jsonObject(with: fixture) as? [String: Any],
    let snapshot = root["snapshot"] as? [[String: Any]], !snapshot.isEmpty
  else { throw BenchError.noSnapshot }
  let turns = snapshot.compactMap { $0["turn"] as? Int }.max() ?? 1
  let blocks = (0..<count).map { index in
    var block = snapshot[index % snapshot.count]
    let copy = index / snapshot.count
    block["id"] = "\(block["id"] as? String ?? "block")~\(copy)"
    // Later turns, so the transcript's realized window (DT§9) collapses old ones as it would.
    block["turn"] = (block["turn"] as? Int ?? 1) + copy * turns
    return block
  }
  return try JSONSerialization.data(withJSONObject: [
    "batches": [[["op": "reset", "blocks": blocks]]], "snapshot": blocks,
  ])
}

/// Whether `windows` (`CGWindowListCopyWindowInfo` rows, on-screen only) hold a window of `pid`
/// the person can see: layer 0, not transparent, not a zero-size helper. Menus and the menu-bar
/// extra sit on higher layers, so they do not end the launch early.
public func showsWindow(_ windows: [[String: Any]], pid: Int32) -> Bool {
  windows.contains { window in
    let bounds = window["kCGWindowBounds"] as? [String: Any]
    return (window["kCGWindowOwnerPID"] as? Int32) == pid
      && (window["kCGWindowLayer"] as? Int) == 0
      && (window["kCGWindowAlpha"] as? Double ?? 1) > 0
      && (bounds?["Width"] as? Double ?? 0) > 1 && (bounds?["Height"] as? Double ?? 0) > 1
  }
}

/// One measurement's runs against its budget. The first run is kept apart: after a build it is
/// the closest to a cold launch this machine gives without `purge`.
public struct Samples: CustomStringConvertible {
  public let name: String
  public let unit: String
  public let budget: Double
  public let values: [Double]

  public init(_ name: String, unit: String, budget: Double, values: [Double]) {
    (self.name, self.unit, self.budget, self.values) = (name, unit, budget, values)
  }

  public var median: Double {
    let sorted = values.sorted()
    guard !sorted.isEmpty else { return .nan }
    let middle = sorted.count / 2
    return sorted.count.isMultiple(of: 2)
      ? (sorted[middle - 1] + sorted[middle]) / 2 : sorted[middle]
  }

  /// The median decides: one run slowed by another process on the machine is not a miss.
  public var withinBudget: Bool { median <= budget }

  public var description: String {
    let runs = values.map { String(format: "%.0f", $0) }.joined(separator: ",")
    return String(
      format: "%@: median=%.0f%@ first=%.0f%@ max=%.0f%@ runs=[%@] budget=%.0f%@ %@", name, median,
      unit, values.first ?? .nan, unit, values.max() ?? .nan, unit, runs, budget, unit,
      withinBudget ? "ok" : "OVER")
  }
}
