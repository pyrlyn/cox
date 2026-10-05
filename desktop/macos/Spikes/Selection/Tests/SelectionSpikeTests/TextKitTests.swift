// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Candidate B measurements: our TextKit 2 view on the 2 000-block fixture.

import AppKit
import QuartzCore
import SelectionSpike
import Testing

@MainActor
@Suite(.serialized)
struct TextKitTests {
  /// Window point at the middle of the first line of `block`'s content, starting `offset`
  /// characters in.
  func point(in view: TranscriptTextView, window: NSWindow, block: Int, offset: Int = 3)
    -> NSPoint
  {
    let range = NSRange(location: view.blockRanges[block].location + offset, length: 1)
    let screen = view.firstRect(forCharacterRange: range, actualRange: nil)
    let rect = window.convertFromScreen(screen)
    return NSPoint(x: rect.midX, y: rect.midY)
  }

  func host(_ blocks: [Block]) -> (Offscreen, NSScrollView, TranscriptTextView) {
    let screen = Offscreen()
    let (scroll, view) = TranscriptTextView.make(blocks: blocks, size: screen.window.frame.size)
    screen.window.contentView = scroll
    screen.flush()
    return (screen, scroll, view)
  }

  func selectedBlocks(_ view: TranscriptTextView) -> [Int] {
    let range = view.selectedRange()
    return view.blockRanges.indices.filter {
      NSIntersectionRange(view.blockRanges[$0], range).length > 0
    }
  }

  @Test func dragSelectsAcrossProseCodeAndProse() {
    let (screen, _, view) = host(Fixture.blocks())
    defer { screen.close() }
    screen.dispatchedDrag(
      from: point(in: view, window: screen.window, block: 0),
      to: point(in: view, window: screen.window, block: 2, offset: 20))
    let blocks = selectedBlocks(view)
    let markdown = view.markdownForSelection()
    measure("textkit drag 0→2 selected blocks", "\(blocks)")
    #expect(blocks == [0, 1, 2])
    #expect(markdown.contains("```swift\nlet value0 = compute(block: 1"))
    let code = markdown.range(of: "compute(block: 1")
    let prose = markdown.range(of: "Block 2 sentence")
    #expect(code != nil && prose != nil && code!.lowerBound < prose!.lowerBound)
  }

  @Test func dragOverToolCardCopiesItsMarkdownInOrder() {
    let (screen, _, view) = host(Fixture.blocks())
    defer { screen.close() }
    screen.dispatchedDrag(
      from: point(in: view, window: screen.window, block: 3, offset: 20),
      to: point(in: view, window: screen.window, block: 5, offset: 20))
    let blocks = selectedBlocks(view)
    let board = NSPasteboard.withUniqueName()
    defer { board.releaseGlobally() }
    _ = view.writeSelection(to: board, types: [.string])
    let pasted = board.string(forType: .string) ?? ""
    measure("textkit drag 3→5 selected blocks", "\(blocks)")
    measure("textkit pasteboard (first 160 chars)", String(pasted.prefix(160)).debugDescription)
    #expect(blocks == [3, 4, 5])
    #expect(pasted.hasPrefix("```diff\n"))
    let card = pasted.range(of: "> **bash** `cargo nextest run -p cox-core -- turn_4")
    let prose = pasted.range(of: "Block 5 sentence")
    #expect(card != nil && prose != nil && card!.lowerBound < prose!.lowerBound)
  }

  @Test func switchOffClampsTheDragToTheFirstBlock() {
    let (screen, _, view) = host(Fixture.blocks())
    defer { screen.close() }
    view.crossBlockSelection = false
    screen.dispatchedDrag(
      from: point(in: view, window: screen.window, block: 0),
      to: point(in: view, window: screen.window, block: 2, offset: 20))
    let blocks = selectedBlocks(view)
    measure("textkit clamp drag 0→2 selected blocks", "\(blocks)")
    #expect(blocks == [0])
    #expect(view.selectedRange().length > 0)
    // Backwards drag clamps to the block it started in, too.
    screen.dispatchedDrag(
      from: point(in: view, window: screen.window, block: 2, offset: 20),
      to: point(in: view, window: screen.window, block: 0))
    measure("textkit clamp drag 2→0 selected blocks", "\(selectedBlocks(view))")
    #expect(selectedBlocks(view) == [2])
  }

  @Test(arguments: [2_000, 10_000])
  func firstFrameAndScroll(count: Int) {
    let blocks = Fixture.blocks(count)
    let before = footprintMB()
    let screen = Offscreen()
    defer { screen.close() }
    let start = CACurrentMediaTime()
    let (scroll, view) = TranscriptTextView.make(blocks: blocks, size: screen.window.frame.size)
    let built = milliseconds(since: start)
    screen.window.contentView = scroll
    screen.flush()
    let firstFrame = milliseconds(since: start)
    measure(
      "textkit \(count) first frame",
      String(format: "%.1f ms (attributed string + storage %.1f ms)", firstFrame, built))
    #expect(view.textLayoutManager != nil, "stayed on TextKit 2")
    let stats = screen.measureScroll(scroll, steps: 1_500, delta: 40)
    measure("textkit \(count) scroll 1500×40pt", stats.description)
    measure(
      "textkit \(count) scrolled to",
      "\(scroll.contentView.bounds.origin.y) pt of \(view.frame.height) pt")
    measure("textkit \(count) footprint delta", String(format: "%.0f MB", footprintMB() - before))
    #expect(view.textLayoutManager != nil, "stayed on TextKit 2 after scrolling")
  }
}
