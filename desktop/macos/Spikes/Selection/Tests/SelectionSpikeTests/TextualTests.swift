// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Candidate A measurements: Textual 0.5.0 on the same fixture. `@testable` reaches
// Textual's internal selection model so the test reads what a drag selected and what
// its Copy command would write, without touching the user's general pasteboard.

import AppKit
import QuartzCore
import SelectionSpike
import SwiftUI
import Testing

@testable import Textual

@MainActor
@Suite(.serialized)
struct TextualTests {
  func host<V: View>(_ view: V, timeout: TimeInterval = 120) -> (Offscreen, Double, Bool) {
    let screen = Offscreen()
    let start = CACurrentMediaTime()
    screen.window.contentView = NSHostingView(rootView: view)
    let ready = screen.wait(timeout: timeout) {
      guard let root = screen.window.contentView else { return false }
      return descendants(of: root, as: NSTextInteractionView.self).contains { $0.model.hasText }
    }
    return (screen, milliseconds(since: start), ready)
  }

  func interactionViews(_ screen: Offscreen) -> [NSTextInteractionView] {
    guard let root = screen.window.contentView else { return [] }
    return descendants(of: root, as: NSTextInteractionView.self)
      .filter { $0.model.hasText && $0.window != nil }
      .sorted { $0.convert($0.bounds, to: nil).maxY > $1.convert($1.bounds, to: nil).maxY }
  }

  /// What each interaction view has selected, in window order.
  func selections(_ screen: Offscreen) -> [String] {
    interactionViews(screen).compactMap { view in
      view.model.selectedRange.map { view.model.text(in: $0) }
    }.filter { !$0.isEmpty }
  }

  /// Block ids the selected text shows: prose "Block N sentence" or "turn_N step", code
  /// "compute(block: N", diff "turn_N()", tool card "-- turn_N ·".
  func blockIds(_ text: String) -> [Int] {
    let pattern =
      /(?:Block (\d+) sentence|turn_(\d+) step|compute\(block: (\d+)|turn_(\d+)\(\)|-- turn_(\d+) )/
    var ids: [Int] = []
    for match in text.matches(of: pattern) {
      let digits = match.1 ?? match.2 ?? match.3 ?? match.4 ?? match.5 ?? ""
      if let id = Int(digits), ids.last != id { ids.append(id) }
    }
    return ids
  }

  @Test func blocksShapeDragStopsAtTheFirstBlock() {
    let (screen, _, ready) = host(TextualBlocksTranscript(blocks: Fixture.blocks()))
    defer { screen.close() }
    #expect(ready)
    let views = interactionViews(screen)
    measure("textual blocks: interaction views realised", "\(views.count)")
    #expect(views.count >= 3)
    guard views.count >= 3 else { return }
    let first = views[0].convert(views[0].bounds, to: nil)
    let third = views[2].convert(views[2].bounds, to: nil)
    screen.dispatchedDrag(
      from: NSPoint(x: first.minX + 20, y: first.maxY - 8),
      to: NSPoint(x: third.minX + 120, y: third.maxY - 8))
    let selected = selections(screen)
    let ids = selected.flatMap(blockIds)
    measure("textual blocks drag 0→2: selections", "\(selected.count), blocks \(ids)")
    // Each block is its own selection context: the drag cannot leave block 0.
    #expect(selected.count == 1)
    #expect(ids == [0])
  }

  @Test func documentShapeDragCrossesBlocksButCopiesPlainTextAndHTML() {
    let (screen, _, ready) = host(TextualDocumentTranscript(blocks: Fixture.blocks()))
    defer { screen.close() }
    #expect(ready)
    guard let view = interactionViews(screen).first else { return }
    let frame = view.convert(view.bounds, to: nil)
    let top = min(frame.maxY, screen.window.frame.height) - 24
    screen.dispatchedDrag(
      from: NSPoint(x: frame.minX + 20, y: top),
      to: NSPoint(x: frame.minX + 200, y: top - 520))
    guard let range = view.model.selectedRange else {
      Issue.record("no selection")
      return
    }
    let attributed = view.model.attributedText(in: range)
    let formatter = Formatter(attributed)
    let plain = formatter.plainText()
    let html = formatter.html()
    measure("textual document drag: blocks in selection", "\(blockIds(plain))")
    measure("textual document copy .string (first 160)", String(plain.prefix(160)).debugDescription)
    measure("textual document copy .html (first 120)", String(html.prefix(120)).debugDescription)
    #expect(blockIds(plain).count >= 3)
    // Copy writes plain text and HTML (NSTextInteractionView.copy); no Markdown fences or marks.
    #expect(!plain.contains("```") && !plain.contains("**"))
  }

  @Test(arguments: [2_000, 10_000])
  func blocksShapeFirstFrameAndScroll(count: Int) {
    let before = footprintMB()
    let (screen, firstFrame, ready) = host(TextualBlocksTranscript(blocks: Fixture.blocks(count)))
    defer { screen.close() }
    #expect(ready)
    measure("textual blocks \(count) first frame", String(format: "%.1f ms", firstFrame))
    guard let root = screen.window.contentView,
      let scroll = descendants(of: root, as: NSScrollView.self).max(by: {
        $0.frame.height < $1.frame.height
      })
    else {
      Issue.record("no NSScrollView under the SwiftUI ScrollView")
      return
    }
    let stats = screen.measureScroll(scroll, steps: 1_500, delta: 40)
    measure("textual blocks \(count) scroll 1500×40pt", stats.description)
    measure("textual blocks \(count) scrolled to", "\(scroll.contentView.bounds.origin.y) pt")
    measure(
      "textual blocks \(count) footprint delta", String(format: "%.0f MB", footprintMB() - before))
  }

  @Test func documentShapeFirstFrameAndScroll() {
    let before = footprintMB()
    let (screen, firstFrame, ready) = host(TextualDocumentTranscript(blocks: Fixture.blocks()))
    defer { screen.close() }
    #expect(ready)
    measure("textual document 2000 first frame", String(format: "%.1f ms", firstFrame))
    guard let root = screen.window.contentView,
      let scroll = descendants(of: root, as: NSScrollView.self).max(by: {
        $0.frame.height < $1.frame.height
      })
    else {
      Issue.record("no NSScrollView under the SwiftUI ScrollView")
      return
    }
    let stats = screen.measureScroll(scroll, steps: 1_500, delta: 40)
    measure("textual document 2000 scroll 1500×40pt", stats.description)
    measure("textual document 2000 scrolled to", "\(scroll.contentView.bounds.origin.y) pt")
    measure(
      "textual document 2000 footprint delta", String(format: "%.0f MB", footprintMB() - before))
  }
}
