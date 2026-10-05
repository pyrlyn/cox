// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `Spinner` (DS§6.2 row `Spinner`, the mockup's `.spin`): work in progress with no known end —
// a running tool, a background task, a subagent. Separate so every busy row turns the same
// ring at the same speed, and all of them hold still under Reduce Motion (`coxSpin`).

import SwiftUI

/// A `fill.secondary` ring with an `accent` quarter arc that turns.
struct Spinner: View {
  /// The mockup's `.spin`, `width: 12px`: no size token is that small.
  private static let diameter: CGFloat = 12
  /// The mockup's `border: 2px`.
  private static let lineWidth: CGFloat = 2
  /// The mockup's `border-top-color`: a quarter of the ring, centred on the top.
  private static let arc: CGFloat = 0.25
  /// One turn a second; the mockup is a still image and no motion token is a period.
  private static let period: TimeInterval = 1

  var body: some View {
    ZStack {
      Circle().strokeBorder(Color(.fillSecondary), lineWidth: Self.lineWidth)
      Circle()
        .inset(by: Self.lineWidth / 2)
        .trim(from: 0, to: Self.arc)
        .stroke(Color(.accent), lineWidth: Self.lineWidth)
        // `trim` starts at three o'clock; this centres the arc on twelve.
        .rotationEffect(.degrees(-90 - Self.arc * 180))
    }
    .frame(width: Self.diameter, height: Self.diameter)
    .coxSpin(period: Self.period)
    .accessibilityElement()
    .accessibilityLabel("In progress")
  }
}

#Preview { PreviewMatrix { Spinner() } }
