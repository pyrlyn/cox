// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A thought's fold header in both states (T37.23.10's Check, DS§6.3 row `ThinkingDisclosure`):
// "Thinking" while the thought streams, "Thought for 12 s" once its `ThinkingDone` gave the
// duration, in light and dark, Solid, through the SwiftUI view the app hosts.

import AppKit
import CoxClient
import SnapshotTesting
import Testing

private let thoughts: [Block] = [
  Block(
    id: "k1", turn: 1, kind: .thinking(text: "The log shows a late event.", durationMs: 12_000)),
  Block(id: "k2", turn: 1, kind: .thinking(text: "The test sleeps instead of waiting.")),
]

// The reading column's width; the height follows the text.
// swiftlint:disable:next no_literal_size
private let size = NSSize(width: 760, height: 200)

@MainActor
@Suite(.serialized)
struct ThoughtHeaderSnapshotTests {
  @Test(arguments: [false, true])
  func endedAndStreamingThoughtHeaders(dark: Bool) throws {
    let host = Host(thoughts, size: size, dark: dark)
    defer { host.close() }
    host.fitToText()
    // Anti-aliasing differs slightly between machines; a real change moves far more pixels.
    assertSnapshot(
      of: try host.image(), as: .image(precision: 0.995, perceptualPrecision: 0.98),
      named: dark ? "dark-solid" : "light-solid", testName: "endedAndStreamingThoughtHeaders")
  }
}
