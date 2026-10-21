// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Install Command-Line Tool (T37.32.2) in a scratch directory: the link names the bundled CLI,
// a stale link is repointed, and a real file or a Debug bundle without the CLI is refused.

import Foundation
import Testing

@testable import CoxPlatform

private struct Scratch {
  let root: URL
  let app: URL
  let bin: URL

  init() throws {
    root = FileManager.default.temporaryDirectory.appending(path: "cox-cli-\(UUID().uuidString)")
    app = root.appending(path: "Cox.app")
    bin = root.appending(path: "home/.local/bin")
    try FileManager.default.createDirectory(
      at: app.appending(path: "Contents/Helpers"), withIntermediateDirectories: true)
  }

  func bundleTheTool() throws -> URL {
    let tool = CommandLineTool.bundled(in: app)
    try Data("#!/bin/sh\n".utf8).write(to: tool)
    try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: tool.path)
    return tool
  }

  func remove() { try? FileManager.default.removeItem(at: root) }
}

@Test func theLinkNamesTheBundledToolAndMakesItsDirectory() throws {
  let scratch = try Scratch()
  defer { scratch.remove() }
  let tool = try scratch.bundleTheTool()
  let link = try CommandLineTool.link(tool, into: scratch.bin)
  #expect(link == scratch.bin.appending(path: "cox"))
  #expect(try FileManager.default.destinationOfSymbolicLink(atPath: link.path) == tool.path)
  // A second install is a no-op.
  #expect(try CommandLineTool.link(tool, into: scratch.bin) == link)
}

@Test func aLinkToAnOlderAppIsRepointed() throws {
  let scratch = try Scratch()
  defer { scratch.remove() }
  let tool = try scratch.bundleTheTool()
  try FileManager.default.createDirectory(at: scratch.bin, withIntermediateDirectories: true)
  let link = scratch.bin.appending(path: "cox")
  try FileManager.default.createSymbolicLink(
    atPath: link.path, withDestinationPath: "/Applications/Old Cox.app/Contents/Helpers/cox")
  try CommandLineTool.link(tool, into: scratch.bin)
  #expect(try FileManager.default.destinationOfSymbolicLink(atPath: link.path) == tool.path)
}

@Test func aRealFileIsNeverReplaced() throws {
  let scratch = try Scratch()
  defer { scratch.remove() }
  let tool = try scratch.bundleTheTool()
  try FileManager.default.createDirectory(at: scratch.bin, withIntermediateDirectories: true)
  let installed = scratch.bin.appending(path: "cox")
  try Data("install.sh copy".utf8).write(to: installed)
  #expect(throws: CommandLineTool.Failure.occupied(installed)) {
    try CommandLineTool.link(tool, into: scratch.bin)
  }
  #expect(try String(contentsOf: installed, encoding: .utf8) == "install.sh copy")
}

@Test func aBundleWithoutTheToolIsRefused() throws {
  let scratch = try Scratch()
  defer { scratch.remove() }
  let tool = CommandLineTool.bundled(in: scratch.app)
  #expect(throws: CommandLineTool.Failure.notBundled(tool)) {
    try CommandLineTool.link(tool, into: scratch.bin)
  }
  #expect(!FileManager.default.fileExists(atPath: scratch.bin.path))
}
