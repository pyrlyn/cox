// A prompt's turn number in the margin left of its bubble (T37.47; Figma frames 01 and 14):
// the prompt's text carries its turn, and a style with a gutter draws it there while the
// system style draws none, so a narrow window's transcript is unchanged.

import AppKit
import CoxClient
import Foundation
import Testing

@testable import CoxTranscriptText

@MainActor
@Suite(.serialized)
struct TranscriptGutterTests {
  /// A wide window around a centred column, so the gutter is inside the view.
  private func render(gutter: TranscriptStyle.Gutter?) throws -> (NSBitmapImageRep, CGRect) {
    var style = TranscriptStyle.system
    // A test column narrower than the window, not a design size.
    // swiftlint:disable:next no_literal_size
    (style.readingWidth, style.gutter) = (400, gutter)
    let view = TranscriptTextView.make(style: style)
    let window = NSWindow(
      // A window frame far off screen, not a design size.
      // swiftlint:disable:next no_literal_size
      contentRect: NSRect(x: -20_000, y: -20_000, width: 700, height: 300),
      styleMask: [.borderless], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = view.inScrollView(frame: NSRect(origin: .zero, size: window.frame.size))
    view.load([Block(id: "u", turn: 7, kind: .user(text: "Why?", attachments: []))])
    window.orderFrontRegardless()
    window.layoutIfNeeded()
    window.displayIfNeeded()
    defer { window.close() }
    let storage = try #require(view.textStorage)
    #expect(storage.attribute(.transcriptTurn, at: 0, effectiveRange: nil) as? Int == 7)

    let bitmap = try #require(view.bitmapImageRepForCachingDisplay(in: view.bounds))
    view.cacheDisplay(in: view.bounds, to: bitmap)
    let screen = view.firstRect(
      forCharacterRange: NSRange(location: 0, length: 1), actualRange: nil)
    let glyph = view.convert(window.convertFromScreen(screen), from: nil)
    let padding = view.textContainer?.lineFragmentPadding ?? 0
    let bubble = view.textContainerOrigin.x + padding
    // swiftlint:disable:next no_literal_size
    let box = CGRect(x: bubble - 44, y: glyph.minY, width: 32, height: glyph.height)
    return (bitmap, box)
  }

  /// The pixels in `box` (view points, flipped) drawn mostly red.
  private func ink(_ bitmap: NSBitmapImageRep, in box: CGRect) -> Int {
    let scale = CGFloat(bitmap.pixelsWide) / bitmap.size.width
    var count = 0
    for y in Int(box.minY * scale)..<Int(box.maxY * scale) {
      for x in Int(box.minX * scale)..<Int(box.maxX * scale) {
        guard let colour = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else { continue }
        if colour.redComponent - colour.greenComponent > 0.3 { count += 1 }
      }
    }
    return count
  }

  @Test func aPromptDrawsItsTurnInTheGutterLeftOfItsBubble() throws {
    // A test gutter in red, so its ink stands out; the app's is CoxUI's tokens.
    // swiftlint:disable no_literal_font no_literal_size
    let gutter = TranscriptStyle.Gutter(
      font: .systemFont(ofSize: 11), color: .systemRed, width: 32, offset: 44)
    // swiftlint:enable no_literal_font no_literal_size
    let (bitmap, box) = try render(gutter: gutter)
    #expect(ink(bitmap, in: box) > 0, "the number is drawn in \(box)")
    let left = box.offsetBy(dx: -box.width, dy: 0)
    #expect(ink(bitmap, in: left) == 0, "and right-aligned, not past its box")
  }

  @Test func withoutAGutterAPromptDrawsNoNumber() throws {
    let (bitmap, box) = try render(gutter: nil)
    #expect(ink(bitmap, in: box) == 0)
  }
}
