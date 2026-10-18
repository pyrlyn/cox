// Copy this file into a test target that uses swift-snapshot-testing. Source of truth:
// pyrlyn/ci tools/snapshots/swift/VersionedSnapshots.swift. Keep `envPrefix` the only edit.
//
// Snapshots live in `<test dir>/Snapshots/<key>/<TestFile>/`. `key` is the runner's
// `xcode<ver>-<build>-macos<ver>` (same format as tools/snapshots/snapshot_version.py), or
// `<envPrefix>_SNAPSHOT_VERSION` when set. A key without a directory skips the test (never
// fails, never records). `<envPrefix>_SNAPSHOT_RECORD=1` records into the key's directory;
// committing that directory is what adds a version.

import Foundation
import SnapshotTesting
import Testing

enum SnapshotVersion {
  /// The one per-repository edit: `COX` reads COX_SNAPSHOT_VERSION and COX_SNAPSHOT_RECORD.
  static let envPrefix = "COX"

  static let key: String = {
    let env = ProcessInfo.processInfo.environment
    for name in ["\(envPrefix)_SNAPSHOT_VERSION", "SNAPSHOT_VERSION"] {
      if let value = env[name]?.trimmingCharacters(in: .whitespaces), !value.isEmpty {
        return value
      }
    }
    return nativeKey()
  }()

  static var recording: Bool {
    ProcessInfo.processInfo.environment["\(envPrefix)_SNAPSHOT_RECORD"] == "1"
  }

  /// `<test dir>/Snapshots/<key>`, next to the calling test file.
  static func directory(forTestFile file: StaticString) -> URL {
    URL(fileURLWithPath: "\(file)").deletingLastPathComponent()
      .appendingPathComponent("Snapshots").appendingPathComponent(key)
  }

  private static func nativeKey() -> String {
    let os = ProcessInfo.processInfo.operatingSystemVersion
    let macos = "\(os.majorVersion).\(os.minorVersion).\(os.patchVersion)"
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/xcrun")
    process.arguments = ["xcodebuild", "-version"]
    let pipe = Pipe()
    process.standardOutput = pipe
    process.standardError = FileHandle.nullDevice
    guard (try? process.run()) != nil else { return "unknown-macos\(macos)" }
    let text = String(decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
    process.waitUntilExit()
    func field(_ pattern: String) -> String? {
      guard let range = text.range(of: pattern, options: .regularExpression) else { return nil }
      return String(text[range]).split(separator: " ").last.map(String.init)
    }
    guard let version = field(#"(?m)^Xcode\s+\S+"#),
      let build = field(#"(?m)^Build version\s+\S+"#)
    else { return "unknown-macos\(macos)" }
    return "xcode\(version)-\(build)-macos\(macos)"
  }
}

/// `assertSnapshot` against the current version's directory. Skips (cancels the test) when
/// there is none; fails on a mismatch or a missing reference image when there is.
func assertVersionedSnapshot<Value, Format>(
  of value: @autoclosure () throws -> Value, as snapshotting: Snapshotting<Value, Format>,
  named name: String? = nil, fileID: StaticString = #fileID, file: StaticString = #filePath,
  testName: String = #function, line: UInt = #line, column: UInt = #column
) throws {
  let directory = SnapshotVersion.directory(forTestFile: file)
  let recording = SnapshotVersion.recording
  if !recording && !FileManager.default.fileExists(atPath: directory.path) {
    try Test.cancel(
      "no snapshots for \(SnapshotVersion.key); record with \(SnapshotVersion.envPrefix)_SNAPSHOT_RECORD=1 and commit")
  }
  let suite = URL(fileURLWithPath: "\(file)").deletingPathExtension().lastPathComponent
  let failure = verifySnapshot(
    of: try value(), as: snapshotting, named: name, record: recording ? .all : .never,
    snapshotDirectory: directory.appendingPathComponent(suite).path,
    fileID: fileID, file: file, testName: testName, line: line, column: column)
  if let failure {
    Issue.record(
      Comment(rawValue: failure),
      sourceLocation: SourceLocation(
        fileID: "\(fileID)", filePath: "\(file)", line: Int(line), column: Int(column)))
  }
}
