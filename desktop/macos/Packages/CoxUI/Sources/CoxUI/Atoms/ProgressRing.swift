// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ProgressRing(fraction)` (DS§6.2 row `ProgressRing(fraction)`, the mockup's `.ring`): a share
// of something used up — context in the cost capsule. Separate from `Spinner` because it shows
// how far, not that something is busy, and never moves on its own.

import SwiftUI

/// An `accent` arc clockwise from twelve o'clock over a `fill.secondary` ring.
struct ProgressRing: View {
  /// The share filled, clamped to 0…1.
  let fraction: Double

  /// The mockup's `.ring`, `width: 14px`: no size token is that small.
  private static let diameter: CGFloat = 14
  /// The mockup's mask leaves a 4 px hole in the 7 px radius.
  private static let lineWidth: CGFloat = 3

  init(_ fraction: Double) {
    self.fraction = fraction.isNaN ? 0 : min(max(fraction, 0), 1)
  }

  var body: some View {
    ZStack {
      Circle().strokeBorder(Color(.fillSecondary), lineWidth: Self.lineWidth)
      Circle()
        .inset(by: Self.lineWidth / 2)
        .trim(from: 0, to: fraction)
        .stroke(Color(.accent), lineWidth: Self.lineWidth)
        // `trim` starts at three o'clock; the mockup's conic gradient starts at twelve.
        .rotationEffect(.degrees(-90))
    }
    .frame(width: Self.diameter, height: Self.diameter)
    .accessibilityElement()
    .accessibilityValue(Text(fraction, format: .percent.precision(.fractionLength(0))))
  }
}

#Preview("empty") { PreviewMatrix { ProgressRing(PreviewState.fractions[0]) } }
#Preview("part") { PreviewMatrix { ProgressRing(PreviewState.fractions[1]) } }
#Preview("full") { PreviewMatrix { ProgressRing(PreviewState.fractions[2]) } }
