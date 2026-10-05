// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ApprovalCard`'s Edit… (T37.27.6): the card hosted in a window far off screen and driven by
// real clicks and typing — Edit…, a new input in the field, Run edited — sends the edited JSON,
// and a draft that is not JSON sends nothing. Separate from the snapshot suite because it
// presses buttons rather than drawing.

import AppKit
import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite(.serialized, .enabled(if: syntheticMouse, "synthesized mouse events need macOS 27"))
struct ApprovalCardEditTests {
  /// The action row's buttons, leading first: pending, then editing.
  private let edit = 2
  private let runEdited = 0

  @Test func editingTheInputSendsTheEditedJSON() throws {
    let card = EditedCard()
    defer { card.close() }
    try card.click(edit)
    try card.type(#"{"command": "git push origin main"}"#)
    try card.click(runEdited)
    #expect(card.sent == [#"{"command": "git push origin main"}"#])
  }

  @Test func aDraftThatIsNotJSONSendsNothingUntilItParses() throws {
    let card = EditedCard()
    defer { card.close() }
    try card.click(edit)
    try card.type(#"{"command": "#)
    try card.click(runEdited)
    #expect(card.sent.isEmpty)
    try card.type(#"{"command": "ls"}"#)
    try card.click(runEdited)
    #expect(card.sent == [#"{"command": "ls"}"#])
  }
}

/// The grant fixture's card with Edit…, in a window AppKit lays out; what Run edited sends is
/// kept in `sent`.
@MainActor
private final class EditedCard {
  private(set) var sent: [String] = []
  private let window: NSWindow

  init() {
    NSApplication.shared.setActivationPolicy(.accessory)
    window = NSWindow(
      // A window frame far off screen, not a design size.
      // swiftlint:disable:next no_literal_size
      contentRect: NSRect(x: -20_000, y: -20_000, width: Size.readingWidth, height: 400),
      styleMask: [.borderless], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    let card = ApprovalCard(
      PreviewState.approvalGrant, act: { _ in }, edit: { [weak self] in self?.sent.append($0) })
    window.contentView = NSHostingView(rootView: card.frame(width: Size.readingWidth))
    window.orderFrontRegardless()
    settle()
  }

  /// Clicks the `index`th button of the action row, counted from the leading edge. SwiftUI
  /// builds no accessibility tree for a window nothing inspects, so a button is found by the
  /// focus ring AppKit keeps over it.
  func click(_ index: Int) throws {
    let rings = descendants(of: window.contentView)
      .filter { String(describing: Swift.type(of: $0)).contains("FocusRing") }
      .map { $0.convert($0.bounds, to: nil) }
      .sorted { $0.minX < $1.minX }
    let frame = try #require(rings.indices.contains(index) ? rings[index] : nil)
    let point = NSPoint(x: frame.midX, y: frame.midY)
    for (number, type) in [NSEvent.EventType.leftMouseDown, .leftMouseUp].enumerated() {
      let event = NSEvent.mouseEvent(
        with: type, location: point, modifierFlags: [],
        timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
        context: nil, eventNumber: number, clickCount: 1, pressure: type == .leftMouseUp ? 0 : 1)
      if let event { window.sendEvent(event) }
    }
    settle()
  }

  /// Replaces the field's text with `text`, as typing does.
  func type(_ text: String) throws {
    // A field edits through the window's field editor once it has focus.
    let input = try #require(
      descendants(of: window.contentView).first { $0 is NSTextField || $0 is NSTextView })
    window.makeFirstResponder(input)
    let field = try #require(window.firstResponder as? NSTextView)
    field.selectAll(nil)
    field.insertText(text, replacementRange: field.selectedRange())
    settle()
  }

  func close() {
    window.orderOut(nil)
    window.close()
  }

  private func settle() {
    for _ in 0..<5 {
      window.layoutIfNeeded()
      window.displayIfNeeded()
      RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.02))
    }
  }
}

private func descendants(of view: NSView?) -> [NSView] {
  var found: [NSView] = []
  var stack = view.map { [$0] } ?? []
  while let next = stack.popLast() {
    found.append(next)
    stack.append(contentsOf: next.subviews)
  }
  return found
}

/// Whether views can be driven by synthesized mouse events. Before macOS 27, NSTextView and
/// SwiftUI's controls track a press in a modal loop that pulls the drag and the release from the
/// application's queue: sent after the press they never arrive and the test hangs; queued ahead
/// of it, they end the test runner's main run loop and the process exits 0 mid-run.
let syntheticMouse = ProcessInfo.processInfo.isOperatingSystemAtLeast(
  OperatingSystemVersion(majorVersion: 27, minorVersion: 0, patchVersion: 0))
