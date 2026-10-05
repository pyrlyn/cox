// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the progress and token-meter atoms (T37.20.1–T37.20.2): the values
// their `#Preview`s and snapshot tests share, and the frames the flexible ones are shown in.
// Separate from `PreviewState.swift` so the atoms built in parallel add their fixtures without
// editing one file.

import SwiftUI

extension PreviewState {
  /// Empty, the mockup's cost-capsule context share, and full.
  static let fractions = [0, 0.38, 1]

  /// The mockup's tok/s series for one turn: two requests with a pause between them.
  private static let tokensPerSecond: [Double] = [
    0, 0, 18, 52, 66, 74, 70, 63, 69, 77, 72, 58, 40, 0, 0, 0, 34, 61, 70, 73, 68, 71, 75, 71,
  ]

  /// Sparkline series: none yet, the first sample, and a whole turn.
  static let series: [(name: String, samples: [Double])] = [
    ("empty", []), ("one", [tokensPerSecond[4]]), ("full", tokensPerSecond),
  ]

  /// Stacked-bar mixes: an empty context, one part, the mockup's turn, and a full window.
  static let bars: [(name: String, segments: [StackedBar.Segment])] = [
    ("empty", []),
    ("history", [.init(kind: .history, fraction: 0.24)]),
    (
      "turn",
      [
        .init(kind: .system, fraction: 0.03), .init(kind: .tools, fraction: 0.05),
        .init(kind: .instructions, fraction: 0.06), .init(kind: .history, fraction: 0.24),
      ]
    ),
    (
      "full",
      [
        .init(kind: .system, fraction: 0.1), .init(kind: .tools, fraction: 0.2),
        .init(kind: .instructions, fraction: 0.3), .init(kind: .history, fraction: 0.4),
      ]
    ),
  ]
}

/// A sparkline at the size the token popover draws it, the mockup's 150 × 34.
struct SparklineSample: View {
  let samples: [Double]
  let tint: Color

  private static let width: CGFloat = 150
  private static let height: CGFloat = 34

  init(_ samples: [Double], tint: Color = Color(.meterReceived)) {
    self.samples = samples
    self.tint = tint
  }

  var body: some View {
    Sparkline(samples, tint: tint).frame(width: Self.width, height: Self.height)
  }
}

/// A stacked bar at half the token popover's width.
struct StackedBarSample: View {
  let segments: [StackedBar.Segment]

  init(_ segments: [StackedBar.Segment]) {
    self.segments = segments
  }

  var body: some View {
    StackedBar(segments).frame(width: Size.tokenPopoverWidth / 2)
  }
}
