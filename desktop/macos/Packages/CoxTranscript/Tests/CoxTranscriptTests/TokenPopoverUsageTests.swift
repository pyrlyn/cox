// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// T37.25.2 (A98): the core's context share and split become the token popover's heading and its
// bar segments and legend, and a part of a kind this build has no colour role for is skipped.

import CoxClient
import CoxUI
import Testing

@testable import CoxTranscript

@MainActor
@Suite struct TokenPopoverUsageTests {
  @Test func theContextSplitBecomesTheBarAndItsLegend() {
    var text = MeterText()
    (text.context, text.contextShare) = ("Context · 3.7k", "0.4% of 1M")
    text.contextParts = [
      ContextPart(kind: "system", label: "System", tokens: "110", share: 0.0001),
      ContextPart(kind: "tools", label: "Tools", tokens: "3.5k", share: 0.0035),
      ContextPart(kind: "memory", label: "Memory", tokens: "1k", share: 0.001),
      ContextPart(kind: "history", label: "History", tokens: "96", share: 0.0001),
    ]
    let usage = UsageView(
      session: Tally(
        sent: 0, received: 0, cacheRead: 0, cacheWrite: 0, uncached: 0, costUsd: 0, calls: 0,
        estimated: false),
      turn: nil, contextTokens: 3_700, text: text)

    let state = TokenPopover.State(usage, isRunning: false)

    #expect(state.contextShare == "0.4% of 1M")
    #expect(
      state.parts == [
        TokenPopover.Part(kind: .system, fraction: 0.0001, legend: "System 110"),
        TokenPopover.Part(kind: .tools, fraction: 0.0035, legend: "Tools 3.5k"),
        TokenPopover.Part(kind: .history, fraction: 0.0001, legend: "History 96"),
      ])
  }
}
