// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `Spinner` and `ProgressRing`'s check (T37.20.1, DS§6.2): a snapshot per variant × light/dark
// × Solid/Frosted, the spinner turning unless Reduce Motion is on, and the ring clamping its
// fraction.

import Foundation
import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct ProgressAtomTests {
  /// Drawn still, so the image does not depend on how long the first frame took.
  @Test(arguments: Variant.all) func spinner(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane { Spinner().environment(\._accessibilityReduceMotion, true) }, variant,
      named: variant.name)
  }

  @Test(arguments: Variant.all) func progressRing(_ variant: Variant) throws {
    for fraction in PreviewState.fractions {
      try assertCoxSnapshot(
        PreviewPane { ProgressRing(fraction) }, variant,
        named: "\(Int(fraction * 100)).\(variant.name)")
    }
  }

  @Test func spinnerTurns() throws {
    #expect(try spinnerMoves(reduceMotion: false, within: 10))
  }

  /// A turning spinner draws a new frame many times over in this span.
  @Test func spinnerHoldsStillUnderReduceMotion() throws {
    #expect(try !spinnerMoves(reduceMotion: true, within: Motion.durationSlow * 4))
  }

  @Test func progressRingClampsItsFraction() {
    #expect(ProgressRing(1.5).fraction == 1)
    #expect(ProgressRing(-0.5).fraction == 0)
    #expect(ProgressRing(.nan).fraction == 0)
  }

  /// Whether a spinner draws a frame unlike its first within `limit` seconds. The wait ends on
  /// the first changed frame, not on a clock, so load slows the test but cannot fail it
  /// (T37.21: the fixed sampling window could close before a second frame was drawn).
  private func spinnerMoves(reduceMotion: Bool, within limit: TimeInterval) throws -> Bool {
    let host = SnapshotHost(
      PreviewPane { Spinner() }, Variant(scheme: .light, material: .solid),
      reduceMotion: reduceMotion)
    let start = try host.bitmap().tiffRepresentation
    let later = try host.bitmap(until: { $0.tiffRepresentation != start }, limit: limit)
    return later.tiffRepresentation != start
  }
}
