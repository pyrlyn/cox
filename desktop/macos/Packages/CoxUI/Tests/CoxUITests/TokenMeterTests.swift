// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `TokenMeter` and `TokenPopover`'s check (T37.25, DS§6.3–§6.4, mockup screen 30): snapshots of
// the meter idle, streaming and open, the popover streaming, after the turn and fed with the
// core's context split (T37.25.2), and the composer with the popover open, × light/dark ×
// Solid/Frosted; and VoiceOver reading the three numbers.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct TokenMeterTests {
  @Test(arguments: Variant.all) func meter(_ variant: Variant) throws {
    let looks = [
      ("idle", PreviewState.meterIdle, false), ("streaming", PreviewState.meterStreaming, false),
      ("open", PreviewState.meterStreaming, true),
    ]
    for (look, state, isOpen) in looks {
      try assertCoxSnapshot(
        PreviewPane { TokenMeterSample(state: state, isOpen: isOpen).fixedSize() }, variant,
        named: "\(look).\(variant.name)")
    }
  }

  @Test(arguments: Variant.all) func popover(_ variant: Variant) throws {
    let looks = [
      ("streaming", PreviewState.tokensStreaming), ("idle", PreviewState.tokensIdle),
      ("live", PreviewState.tokensLive),
    ]
    for (look, state) in looks {
      try assertCoxSnapshot(
        PreviewPane { TokenPopover(state: state).fixedSize() }, variant,
        named: "\(look).\(variant.name)")
    }
  }

  @Test(arguments: Variant.all) func composer(_ variant: Variant) throws {
    try assertCoxSnapshot(
      ComposerSample(state: PreviewState.composerTokens).fixedSize(), variant,
      named: "open.\(variant.name)")
  }

  /// DS§8: the meter is one element read as the core's spoken line, not as its glyphs. A hosted
  /// view builds no accessibility tree without an assistive client, so the label is read from
  /// the meter itself; `cox_app::MeterText`'s tests hold the wording.
  @Test func voiceOverReadsTheThreeNumbers() {
    let meter = TokenMeter(state: PreviewState.meterStreaming) {}
    #expect(
      meter.accessibilityText
        == "218 thousand tokens sent, 9.8 thousand received, 71 tokens per second")
  }
}
