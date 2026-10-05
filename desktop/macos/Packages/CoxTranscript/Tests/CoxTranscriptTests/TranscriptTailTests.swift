// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Following the tail (T37.23.7's Check): a reply streamed as `docTail` patches through the
// store keeps a real scroll view at the bottom while the reader is there, leaves it put once
// they scroll up, and follows again when they return to the bottom.

import AppKit
import CoxClient
import Testing

private func paragraph(_ text: String) -> DocBlock {
  .text(kind: .paragraph, lines: [TextLine([Span(text: text)])])
}

private func reply(_ id: BlockID, _ text: String) -> Block {
  Block(id: id, turn: 1, kind: .assistant(text: "", doc: StyledDoc(blocks: [paragraph(text)])))
}

private let sentence = "The watcher test sleeps instead of waiting for the first event. "

/// Enough text above the streamed reply to fill several windows.
private let earlier = (0..<12).map { reply("b\($0)", String(repeating: sentence, count: 8)) }

// A reading column's width and a window's height, not design sizes.
// swiftlint:disable:next no_literal_size
private let size = NSSize(width: 760, height: 400)

extension Host {
  /// How far down the text the view is scrolled.
  var offset: CGFloat { scroll?.contentView.bounds.minY ?? 0 }

  /// Whether the view shows the end of the text.
  var atBottom: Bool {
    guard let scroll else { return false }
    return scroll.documentVisibleRect.maxY >= text.bounds.maxY - 1
  }

  /// Scrolls to `y` as a reader's scroll wheel does: the clip view moves, then the scrollers.
  func scroll(to y: CGFloat) {
    guard let scroll else { return }
    scroll.contentView.scroll(to: NSPoint(x: 0, y: y))
    scroll.reflectScrolledClipView(scroll.contentView)
    flush()
  }

  /// The reader's return to the end: scrolls to the text's bottom edge until it stays there, as
  /// TextKit 2 lays out the text it scrolls past and the text's height settles.
  func scrollToBottom() {
    for _ in 0..<5 where !atBottom {
      scroll(to: text.bounds.maxY - (scroll?.contentView.bounds.height ?? 0))
    }
  }
}

@MainActor
@Suite(.serialized)
struct TranscriptTailTests {
  /// Streams `count` more paragraphs into `id`, one `docTail` batch a paragraph, as the core
  /// sends a reply's new last paragraph, with a frame after each.
  private func stream(
    _ host: Host, _ id: BlockID, _ paragraphs: inout [String], count: Int,
    check: () -> Void = {}
  ) {
    for _ in 0..<count {
      paragraphs.append(String(repeating: sentence, count: 3))
      let last = UInt32(paragraphs.count - 1)
      host.store.apply([.docTail(id: id, from: last, blocks: [paragraph(paragraphs.last ?? "")])])
      host.flush()
      check()
    }
  }

  @Test func aStreamedReplyKeepsTheBottomInViewOnlyWhileTheReaderIsThere() throws {
    let streamed: BlockID = "s"
    let host = Host(earlier + [reply(streamed, "Fixed it.")], size: size)
    defer { host.close() }
    host.settle()
    host.scrollToBottom()
    #expect(host.atBottom)
    var paragraphs = ["Fixed it."]

    // At the bottom: every batch keeps the new end in view.
    let before = host.offset
    stream(host, streamed, &paragraphs, count: 6) {
      #expect(host.atBottom, "offset \(host.offset) of \(host.text.bounds.height)")
    }
    #expect(host.offset > before, "the view moved down with the reply")

    // Scrolled up: the view stays where the reader left it.
    host.scroll(to: host.offset - 300)
    let left = host.offset
    #expect(!host.atBottom)
    stream(host, streamed, &paragraphs, count: 4) {
      #expect(host.offset == left)
    }
    #expect(!host.atBottom)

    // Back at the bottom: following resumes.
    host.scrollToBottom()
    #expect(host.atBottom)
    stream(host, streamed, &paragraphs, count: 4) {
      #expect(host.atBottom, "offset \(host.offset) of \(host.text.bounds.height)")
    }
  }

  @Test func aReplyThatOutgrowsAShortTranscriptIsFollowed() throws {
    let streamed: BlockID = "s"
    let host = Host([reply(streamed, "Looking.")], size: size)
    defer { host.close() }
    host.settle()
    var paragraphs = ["Looking."]

    stream(host, streamed, &paragraphs, count: 20)

    #expect(host.text.bounds.height > size.height, "the reply outgrew the window")
    #expect(host.atBottom, "offset \(host.offset) of \(host.text.bounds.height)")
  }

  @Test func aTranscriptOpenedAtTheTopStaysThereWhileAReplyStreams() throws {
    let streamed: BlockID = "s"
    let host = Host(earlier + [reply(streamed, "Fixed it.")], size: size)
    defer { host.close() }
    host.settle()
    var paragraphs = ["Fixed it."]

    stream(host, streamed, &paragraphs, count: 3) {
      #expect(host.offset == 0)
    }
  }
}
