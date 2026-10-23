// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Launches the built app on a 2 000-block fixture and measures two DT§1 budgets from outside
// it (T37.33): launch to the first visible window (≤ 400 ms) and the idle app's memory with that
// session open (≤ 150 MB RSS). From outside, so the app carries no benchmark code and the number
// includes everything a launch pays — dyld, the Rust core's start, SwiftUI's first frame.
// `XCTApplicationLaunchMetric` would need a UI-test target and its runner's Accessibility grant;
// the window server's list needs neither.
//
//   swift run -c release --package-path desktop/macos/Benchmarks cox-bench \
//     desktop/macos/build/Cox.app desktop/macos/Fixtures/edit.json [runs]

import CoreGraphics
import Darwin
import Foundation
import LaunchBench

let arguments = CommandLine.arguments
guard arguments.count >= 3 else {
  FileHandle.standardError.write(Data("usage: cox-bench <Cox.app> <fixture.json> [runs]\n".utf8))
  exit(2)
}
guard let app = Bundle(url: URL(filePath: arguments[1]))?.executableURL else {
  FileHandle.standardError.write(Data("cox-bench: \(arguments[1]) is no app bundle\n".utf8))
  exit(2)
}
let runs = arguments.count > 3 ? max(1, Int(arguments[3]) ?? 5) : 5

let scratch = FileManager.default.temporaryDirectory.appending(path: "cox-bench-\(getpid())")
let home = scratch.appending(path: "home")
let fixture = scratch.appending(path: "2000-blocks.json")

/// `exit` skips `defer`, so every way out after the scratch tree exists goes through here.
func finish(_ code: Int32) -> Never {
  try? FileManager.default.removeItem(at: scratch)
  exit(code)
}
do {
  try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
  try expand(fixture: Data(contentsOf: URL(filePath: arguments[2])), to: 2_000).write(to: fixture)
} catch {
  FileHandle.standardError.write(Data("cox-bench: \(error)\n".utf8))
  finish(1)
}

/// The app's memory as the kernel counts it, in MB: resident set (DT§1's budget) and the
/// footprint Activity Monitor and `XCTMemoryMetric` show.
func memory(of pid: Int32) -> (rss: Double, footprint: Double)? {
  var info = rusage_info_v4()
  let read = withUnsafeMutablePointer(to: &info) { pointer in
    pointer.withMemoryRebound(to: rusage_info_t?.self, capacity: 1) {
      proc_pid_rusage(pid, RUSAGE_INFO_V4, $0)
    }
  }
  guard read == 0 else { return nil }
  let megabyte = 1_048_576.0
  return (Double(info.ri_resident_size) / megabyte, Double(info.ri_phys_footprint) / megabyte)
}

func onScreen() -> [[String: Any]] {
  CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
    as? [[String: Any]] ?? []
}

var launches: [Double] = []
var resident: [Double] = []
var footprints: [Double] = []
for run in 1...runs {
  let process = Process()
  process.executableURL = app
  // No restored windows from an earlier run: every launch opens the one fixture session.
  process.arguments = ["-CoxFixture", fixture.path, "-ApplePersistenceIgnoreState", "YES"]
  // A scratch home and no Keychain, as every development run of cox (A49, A51).
  process.environment = ProcessInfo.processInfo.environment.merging(
    ["COX_HOME": home.path, "COX_KEYRING": "off"]) { $1 }
  process.standardOutput = FileHandle.nullDevice
  process.standardError = FileHandle.nullDevice

  let start = ContinuousClock.now
  do { try process.run() } catch {
    FileHandle.standardError.write(Data("cox-bench: cannot launch \(app.path): \(error)\n".utf8))
    finish(1)
  }
  let deadline = start + .seconds(30)
  while process.isRunning, !showsWindow(onScreen(), pid: process.processIdentifier),
    ContinuousClock.now < deadline
  {
    usleep(1_000)
  }
  let launched = start.duration(to: .now)
  guard process.isRunning else {
    let status = process.terminationStatus
    FileHandle.standardError.write(
      Data("cox-bench: run \(run): the app exited (\(status)) before showing a window\n".utf8))
    finish(1)
  }
  guard launched < .seconds(30) else {
    process.terminate()
    FileHandle.standardError.write(Data("cox-bench: run \(run) showed no window in 30 s\n".utf8))
    finish(1)
  }
  // Idle: the session opened, the fixture's batch folded and the transcript laid out.
  sleep(3)
  let used = memory(of: process.processIdentifier)
  process.terminate()
  process.waitUntilExit()

  let milliseconds =
    Double(launched.components.seconds) * 1_000 + Double(launched.components.attoseconds) / 1e15
  launches.append(milliseconds)
  if let used { (resident, footprints) = (resident + [used.rss], footprints + [used.footprint]) }
  print(
    String(
      format: "run %d: window %.0f ms, rss %.0f MB, footprint %.0f MB", run, milliseconds,
      used?.rss ?? .nan, used?.footprint ?? .nan))
}

var load = [Double](repeating: 0, count: 3)
let loadText = getloadavg(&load, 3) == 3 ? String(format: "%.1f", load[0]) : "?"
let launch = Samples("launch to window", unit: "ms", budget: 400, values: launches)
let rss = Samples("idle rss 2000 blocks", unit: "MB", budget: 150, values: resident)
print("MEASURE \(launch) load=\(loadText)")
print("MEASURE \(rss) load=\(loadText)")
print(
  String(
    format: "MEASURE idle footprint 2000 blocks: median=%.0fMB (reported, not budgeted)",
    Samples("footprint", unit: "MB", budget: 0, values: footprints).median))
finish(launch.withinBudget && rss.withinBudget ? 0 : 1)
