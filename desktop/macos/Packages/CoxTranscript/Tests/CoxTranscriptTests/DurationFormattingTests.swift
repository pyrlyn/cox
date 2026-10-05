// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A tool card's duration (T37.44.17): verification found it printed as "0,0s" in a
// comma-decimal locale because every duration was forced through a fixed one-decimal seconds
// format. A duration under a second now prints as milliseconds, and one at or above a second
// drops the decimal when it would just be zero — in whatever locale the reader is in.

import CoxClient
import CoxUI
import Foundation
import Testing

@testable import CoxTranscript

private func toolBlock(durationMs: UInt64) -> Block {
  Block(
    id: "t", turn: 1,
    kind: .tool(
      tool: "bash", summary: "ls", icon: .shell, risk: .readOnly, state: .done, tail: "",
      archive: nil, diff: nil, durationMs: durationMs))
}

private func duration(_ ms: UInt64, _ localeID: String) -> String? {
  ToolCard.Content(toolBlock(durationMs: ms), locale: Locale(identifier: localeID))?.header
    .duration
}

@Test func subSecondDurationsPrintAsMillisecondsNotAStrayDecimal() {
  #expect(duration(0, "en_US") == "0ms")
  #expect(duration(40, "en_US") == "40ms")
  #expect(duration(999, "en_US") == "999ms")
  #expect(duration(0, "ru_RU") == "0 мс")
  #expect(duration(40, "ru_RU") == "40 мс")
}

@Test func aDurationThatRoundsToAWholeSecondDropsTheDecimal() {
  #expect(duration(4000, "en_US") == "4s")
  #expect(duration(2999, "en_US") == "3s")
  #expect(duration(4000, "ru_RU") == "4 с")
}

@Test func aFractionalSecondDurationShowsAtMostOneDecimalInTheLocale() {
  #expect(duration(2400, "en_US") == "2.4s")
  #expect(duration(2400, "ru_RU") == "2,4 с")
}
