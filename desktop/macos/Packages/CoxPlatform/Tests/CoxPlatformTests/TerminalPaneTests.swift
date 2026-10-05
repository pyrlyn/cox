// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The terminal pane's bridge over a fake `TerminalClient` (T51.5): keys typed into SwiftTerm's
// view reach `write`, a new frame reaches `resize` in cells, and bytes the shell prints appear
// in the view's buffer. No process is spawned; the fake is the whole shell.

import AppKit
import CoxClient
import SwiftTerm
import Synchronization
import Testing

@testable import CoxPlatform

/// Records what the view sends and hands out what a test yields.
final class FakeTerminal: TerminalClient {
  struct Size: Equatable, Sendable {
    var cols: UInt16
    var rows: UInt16
  }

  let written = Mutex<[UInt8]>([])
  let sizes = Mutex<[Size]>([])
  let outputs: AsyncStream<[UInt8]>
  let shell: AsyncStream<[UInt8]>.Continuation

  init() {
    (outputs, shell) = AsyncStream.makeStream(of: [UInt8].self)
  }

  func write(_ bytes: [UInt8]) throws { written.withLock { $0 += bytes } }
  func resize(cols: UInt16, rows: UInt16) throws {
    sizes.withLock { $0.append(Size(cols: cols, rows: rows)) }
  }
  func exitStatus() -> UInt32? { nil }
  func isBusy() -> Bool { false }
  func close() { shell.finish() }
}

// swiftlint:disable:next no_literal_size
private let frame = NSRect(x: 0, y: 0, width: 640, height: 320)
// swiftlint:disable:next no_literal_size
private let wider = NSSize(width: 960, height: 540)
// swiftlint:disable:next no_literal_font
@MainActor private let font = NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)

@MainActor
private func mounted(_ client: FakeTerminal) -> (TerminalView, TerminalBridge) {
  let view = TerminalView(frame: frame, font: font)
  let bridge = TerminalBridge(client: client, openLink: { _ in })
  bridge.attach(view)
  // The view holds its delegate weakly; the caller keeps the bridge.
  return (view, bridge)
}

@MainActor @Test func typedKeysReachTheShellsWrite() {
  let fake = FakeTerminal()
  let (view, bridge) = mounted(fake)
  view.insertText("ls -la\r", replacementRange: NSRange(location: NSNotFound, length: 0))
  #expect(fake.written.withLock { $0 } == Array("ls -la\r".utf8))
  bridge.detach()
}

@MainActor @Test func aNewFrameReachesResizeInCells() {
  let fake = FakeTerminal()
  let (view, bridge) = mounted(fake)
  view.setFrameSize(wider)
  let terminal = view.getTerminal()
  let last = fake.sizes.withLock { $0.last }
  #expect(
    last == FakeTerminal.Size(cols: UInt16(terminal.cols), rows: UInt16(terminal.rows)))
  #expect((last?.cols ?? 0) > 80, "a wider frame is more columns")
  bridge.detach()
}

@MainActor @Test func bytesTheShellPrintsAppearInTheBuffer() async throws {
  let fake = FakeTerminal()
  let (view, bridge) = mounted(fake)
  fake.shell.yield(Array("hello from the shell\r\n".utf8))
  var text = ""
  for _ in 0..<200 where !text.contains("hello from the shell") {
    try await Task.sleep(for: .milliseconds(10))
    text = String(bytes: view.getTerminal().getBufferAsData(), encoding: .utf8) ?? ""
  }
  #expect(text.contains("hello from the shell"))
  bridge.detach()
}

/// What the caller's `openLink` was handed.
@MainActor
private final class Links {
  var opened: [String] = []
}

@MainActor @Test func aClickedLinkGoesToTheCallerNotTheWorkspace() {
  let fake = FakeTerminal()
  let links = Links()
  let view = TerminalView(frame: frame, font: nil)
  let bridge = TerminalBridge(client: fake, openLink: { links.opened.append($0) })
  bridge.attach(view)
  bridge.requestOpenLink(source: view, link: "file:///etc/passwd", params: [:])
  #expect(links.opened == ["file:///etc/passwd"])
  #expect(bridge.clipboardRead(source: view) == nil)
  bridge.detach()
}

/// A tab's surface feeds its own view with no pane on screen, so a hidden tab keeps its output.
@MainActor @Test func aSurfaceFeedsItsViewWhileNoPaneShowsIt() async throws {
  let fake = FakeTerminal()
  let surface = TerminalSurface(
    client: fake,
    style: TerminalStyle(font: font, foreground: .white, background: .black, caret: .white))
  fake.shell.yield(Array("printed while hidden\r\n".utf8))
  var text = ""
  for _ in 0..<200 where !text.contains("printed while hidden") {
    try await Task.sleep(for: .milliseconds(10))
    text = String(bytes: surface.view.getTerminal().getBufferAsData(), encoding: .utf8) ?? ""
  }
  #expect(text.contains("printed while hidden"))
  surface.end()
}

/// The style's line height reaches SwiftTerm, so rows sit at the token's pitch rather than the
/// font's tighter own.
@MainActor @Test func theStylesLineHeightSetsTheRowPitch() {
  let view = TerminalView(frame: frame, font: font)
  TerminalStyle(font: font, foreground: .white, background: .black, caret: .white, lineHeight: 18)
    .apply(to: view)
  let natural = font.ascender - font.descender + font.leading
  #expect(abs(view.lineSpacing * natural - 18) < 0.01)
}
