// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The DT§9 benchmark gate (T37.23): a 2 000-block transcript through `TranscriptView`, with
// `cross_block_selection` on and a live selection across blocks, holds DT§1's budgets —
// scrolling 1 500 frames × 40 pt within 1 % hitch time, and a reply streamed at 200 tok/s at the
// bottom within 25 % main-thread busy time and no frame over 16 ms. A frame is main-thread
// layout, draw and commit, as spike T37.37 timed it (research.md §9.5.13); the render server's
// GPU time is not in it. Timed on the dev machine the suite runs on, not the M1 Air DT§1 names,
// so a loaded machine can fail it; the MEASURE lines say by how much.

import AppKit
import CoxClient
import QuartzCore
import Testing

private func span(_ text: String, _ token: StyleToken = .text, bold: Bool = false) -> Span {
  var span = Span(text: text)
  (span.token, span.bold) = (token, bold)
  return span
}

private func paragraph(_ text: String) -> DocBlock {
  .text(kind: .paragraph, lines: [TextLine([span(text)])])
}

private func reply(_ id: BlockID, _ blocks: [DocBlock]) -> Block {
  Block(id: id, turn: 1, kind: .assistant(text: "", doc: StyledDoc(blocks: blocks)))
}

/// Prose, code, prose, diff and a finished tool call as a CoxUI card, in rotation — the shape
/// spike T37.37 timed, with real cards in place of summary lines.
private func transcript(_ count: Int) -> [Block] {
  (0..<count).map { index in
    let id = "b\(index)"
    switch index % 5 {
    case 0, 2:
      let sentences = (0..<(2 + index % 5)).flatMap { sentence in
        [
          span("Block \(index) sentence \(sentence) explains "), span("why", bold: true),
          span(" the "), span("turn_\(index)", .accent),
          span(" step ran before the compaction and what the model saw next. "),
        ]
      }
      return reply(id, [.text(kind: .paragraph, lines: [TextLine(sentences)])])
    case 1:
      let lines = (0..<(3 + index % 9)).map { line in
        [span("let value\(line) = compute(block: \(index), line: \(line)) // step \(line)")]
      }
      return reply(id, [.code(lang: "swift", lines: lines)])
    case 3:
      let lines = [
        [span("@@ -\(index),3 +\(index),3 @@", .diffHunk)], [span(" fn turn_\(index)() {")],
        [span("-    let budget = \(index);", .diffDel)],
        [span("+    let budget = \(index + 1);", .diffAdd)], [span(" }")],
      ]
      return reply(id, [.code(lang: "diff", lines: lines)])
    default:
      return Block(
        id: id, turn: 1,
        kind: .tool(
          tool: "bash", summary: "Ran cargo nextest run -p cox-core -- turn_\(index)", icon: .shell,
          risk: .exec, state: .done, tail: "    Starting 38 tests\n        PASS turn_\(index)\n",
          archive: nil, diff: nil, durationMs: 1_200))
    }
  }
}

/// Per-frame main-thread times, in milliseconds.
struct FrameStats: CustomStringConvertible {
  static let budget = 1_000.0 / 60.0
  let samples: [Double]

  init(_ samples: [Double]) { self.samples = samples.sorted() }

  func percentile(_ share: Double) -> Double {
    samples.isEmpty ? 0 : samples[min(samples.count - 1, Int(Double(samples.count) * share))]
  }

  var max: Double { samples.last ?? 0 }

  /// Time over the 16.7 ms budget as a share of the frames' budgeted time — the headless
  /// stand-in for XCTest's hitch-time ratio.
  var hitchRatio: Double {
    samples.reduce(0) { $0 + Swift.max(0, $1 - Self.budget) } / budgeted
  }

  /// Main-thread time as a share of the frames' budgeted time.
  var busy: Double { samples.reduce(0, +) / budgeted }

  private var budgeted: Double { Double(samples.count) * Self.budget }

  var description: String {
    String(
      format: "frames=%d p50=%.2fms p95=%.2fms p99=%.2fms max=%.2fms hitch=%.2f%% busy=%.1f%%",
      samples.count, percentile(0.5), percentile(0.95), percentile(0.99), max, hitchRatio * 100,
      busy * 100)
  }
}

extension Host {
  /// Runs `step` then one frame drawing `dirty` (everything shown when `nil`), `count` times,
  /// timing each.
  func frames(
    _ count: Int, drawing dirty: () -> NSRect? = { nil }, step: (Int) -> Void
  ) -> FrameStats {
    FrameStats(
      (0..<count).map { index in
        let start = CACurrentMediaTime()
        step(index)
        flush(drawing: dirty())
        return (CACurrentMediaTime() - start) * 1_000
      })
  }

  /// A drag from the first block into the fourth, so a selection spans blocks while it runs.
  /// Where a drag cannot be synthesized, the range it selects is set instead.
  func selectAcrossBlocks() {
    guard syntheticMouse else {
      let start = (text.range(of: "b0")?.location ?? 0) + 2
      let end = (text.range(of: "b3")?.location ?? 0) + 4
      text.setSelectedRange(NSRange(location: start, length: end - start))
      return
    }
    drag(from: point("b0", 2), to: point("b3", 4))
  }
}

/// DT§1's budgets hold on the hardware the app ships to. A CI runner is a VM whose paravirtual
/// GPU draws several times slower, so there a miss is a known issue, the MEASURE line left to
/// read, instead of a failed job.
private func withinBudget(_ body: () -> Void) {
  if ProcessInfo.processInfo.environment["CI"] == nil {
    body()
  } else {
    withKnownIssue("DT§1 budgets are timed on a Mac, not a CI VM", isIntermittent: true, body)
  }
}

/// Machine load, for reading a failure.
private var load: String {
  var sample = [Double](repeating: 0, count: 3)
  return getloadavg(&sample, 3) == 3 ? String(format: "%.1f", sample[0]) : "?"
}

// The reading column's width and a window's height, not design sizes.
// swiftlint:disable:next no_literal_size
private let size = NSSize(width: 760, height: 800)

@MainActor
@Suite(.serialized)
struct TranscriptBenchmarkTests {
  @Test func scrollingTwoThousandBlocksKeepsHitchTimeUnderOnePercent() throws {
    let host = Host(transcript(2_000), size: size)
    defer { host.close() }
    host.settle()
    host.selectAcrossBlocks()
    #expect(host.selectedBlocks == ["b0", "b1", "b2", "b3"])
    let scroll = try #require(host.scroll)
    let clip = scroll.contentView

    let stats = host.frames(1_500) { _ in
      clip.scroll(to: NSPoint(x: clip.bounds.minX, y: clip.bounds.minY + 40))
      scroll.reflectScrolledClipView(clip)
    }

    print("MEASURE scroll 2000 blocks: \(stats) load=\(load)")
    #expect(clip.bounds.minY > 40 * 1_000, "the scroll ran through the transcript")
    withinBudget { #expect(stats.hitchRatio <= 0.01, "DT§1: hitch time ≤ 1 %, \(stats)") }
  }

  @Test func streamingAtTwoHundredTokensASecondKeepsTheMainThreadMostlyFree() throws {
    let streamed: BlockID = "s"
    let host = Host(transcript(2_000) + [reply(streamed, [paragraph("")])], size: size)
    defer { host.close() }
    host.settle()
    host.selectAcrossBlocks()
    host.text.scrollToEndOfDocument(nil)
    host.settle()

    // 200 tok/s at 60 frames a second, a token about a word, a paragraph every 60 tokens; each
    // frame sends what the core would: the reply's last paragraph as it now reads, which the view
    // follows down itself (T37.23.7). A frame draws the growing paragraph down, as a window on
    // screen redraws only what changed; redrawing all 800 pt shown each frame adds about 7 ms
    // that no real frame spends.
    var paragraphs = [""]
    var sent = 0
    func send(_ frame: Int) {
      let due = (frame + 1) * 200 / 60
      while sent < due {
        if sent > 0, sent.isMultiple(of: 60) { paragraphs.append("") }
        paragraphs[paragraphs.count - 1] += "token\(sent) "
        sent += 1
      }
      let last = paragraphs.count - 1
      host.store.apply([
        .docTail(id: streamed, from: UInt32(last), blocks: [paragraph(paragraphs[last])])
      ])
    }
    let stats = host.frames(300, drawing: { host.streamedTail }, step: send)

    print("MEASURE stream 200 tok/s after 2000 blocks: \(stats) load=\(load)")
    let text = try #require(host.text.range(of: streamed))
    #expect(text.length > 4_000, "the reply grew in the text")
    #expect(host.selectedBlocks == ["b0", "b1", "b2", "b3"], "the selection survived the stream")
    withinBudget {
      #expect(stats.busy <= 0.25, "DT§1: main thread busy ≤ 25 %, \(stats)")
      #expect(stats.max <= 16, "DT§1: no frame over 16 ms, \(stats)")
    }
  }
}
