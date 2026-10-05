// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `[desktop.transcript]`'s `text_size` and `line_height` reach the transcript (T37.23.13's
// Check): the rows the settings view holds, read through `SettingsStore.transcript`, set the
// prose size and the space between its lines, restyled in place over the text already shown,
// and the transcript is shown at two sizes and line heights.

import AppKit
import CoxClient
import CoxModel
import SnapshotTesting
import Testing

private func line(_ text: String) -> [Span] { [Span(text: text)] }

private let wrapped =
  "The test raced the watcher: it slept for a fixed time, so a slow machine missed the event "
  + "and a fast one waited for nothing. It now waits for the first event instead."

private let blocks: [Block] = [
  Block(id: "u", turn: 1, kind: .user(text: "Why is the watcher test flaky?", attachments: [])),
  Block(
    id: "a", turn: 1,
    kind: .assistant(
      text: "",
      doc: StyledDoc(blocks: [
        .text(kind: .heading(2), lines: [TextLine(line("The fix"))]),
        .text(kind: .paragraph, lines: [TextLine(line(wrapped))]),
        .text(kind: .list, lines: [TextLine(line("wait for the first event"), marker: "•")]),
        .code(lang: "rust", lines: [line("let event = rx.recv().await?;"), line("drop(rx);")]),
      ]))),
]

// The reading column's width and a window's height, not design sizes.
// swiftlint:disable:next no_literal_size
private let size = NSSize(width: 520, height: 300)

/// `[desktop.transcript]` as `cox_app::settings` exports it, set off its defaults.
private func settings(textSize: String, lineHeight: String) -> SettingsView {
  func row(_ field: String, _ value: String) -> Setting {
    Setting(
      key: "desktop.transcript.\(field)", value: value, layer: .user, editable: true,
      kind: field == "cross_block_selection" ? .toggle : .number(min: nil, max: nil),
      description: "")
  }
  return SettingsView(
    settings: [
      row("cross_block_selection", "true"), row("line_height", lineHeight),
      row("text_size", textSize),
    ],
    userFile: "/home/.cox/config.toml")
}

@MainActor
@Suite(.serialized)
struct TranscriptLineHeightTests {
  /// The prose paragraph's font and the space under each of its lines.
  private func prose(_ host: Host) throws -> (font: NSFont, spacing: CGFloat) {
    let start = try #require(host.text.range(of: "a")).location + "The fix\n".utf16.count
    let storage = try #require(host.text.textStorage)
    let font = try #require(storage.attribute(.font, at: start, effectiveRange: nil) as? NSFont)
    let style = storage.attribute(.paragraphStyle, at: start, effectiveRange: nil)
    return (font, try #require(style as? NSParagraphStyle).lineSpacing)
  }

  @Test func theStoredTextSizeAndLineHeightRestyleTheShownText() async throws {
    let store = SettingsStore(
      client: FixtureSettingsClient(view: settings(textSize: "16.0", lineHeight: "2.0")),
      secrets: MemorySecretStore(), cwd: "/project")
    await store.load()
    let transcript = try #require(store.transcript)
    let host = Host(blocks, size: size)
    defer { host.close() }
    host.settle()
    let before = try prose(host)
    let ranges = host.text.blockRanges

    host.show(
      crossBlockSelection: true, textScale: 1.2,
      text: (transcript.textSize, transcript.lineHeight))
    host.settle()

    let after = try prose(host)
    let font = after.font
    #expect(abs(font.pointSize - 16 * 1.2) < 0.01)
    let natural = font.ascender - font.descender + font.leading
    #expect(abs(after.spacing - (font.pointSize * 2 - natural)) < 0.01)
    #expect(after.spacing > before.spacing)
    #expect(host.text.blockRanges == ranges)
  }

  @Test(arguments: [(13.5, 1.55), (17, 2)])
  func atTwoSizesAndLineHeights(textSize: Double, lineHeight: Double) throws {
    let host = Host(blocks, size: size)
    defer { host.close() }
    host.show(crossBlockSelection: true, text: (textSize, lineHeight))
    host.fitToText()
    // Anti-aliasing differs slightly between machines; a real change moves far more pixels.
    assertSnapshot(
      of: try host.image(), as: .image(precision: 0.995, perceptualPrecision: 0.98),
      named: "\(Int(textSize * 10))-\(Int(lineHeight * 100))",
      testName: "atTwoSizesAndLineHeights")
  }
}
