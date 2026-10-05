// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `Sparkline(samples)` (DS§6.2 row `Sparkline(samples)`, the mockup's `svg` in `.meter`): the
// shape of a recent series — tok/s in the token meter and its popover. Separate so the meter
// and the popover draw one series the same way at any size; the caller sets the frame.

import SwiftUI

/// A tinted line over a fill that fades from the tint to nothing, the largest sample at the
/// top. Decorative: the meter says the numbers in words (DS§8).
struct Sparkline: View {
  let samples: [Double]
  let tint: Color

  /// The mockup's `stroke-width="1.5"`.
  private static let lineWidth: CGFloat = 1.5
  /// The mockup's gradient, `stop-opacity=".35"` at the top.
  private static let fillOpacity = 0.35
  /// The mockup's `h-1-v/mx*(h-3)`: 1 pt below the lowest sample, 2 pt above the highest,
  /// so the line is never cut by the frame.
  nonisolated private static let insets = (top: CGFloat(2), bottom: CGFloat(1))

  /// `tint` defaults to `meter.received`: tok/s counts received tokens.
  init(_ samples: [Double], tint: Color = Color(.meterReceived)) {
    self.samples = samples
    self.tint = tint
  }

  var body: some View {
    ZStack {
      SparklinePath(samples: samples, closed: true)
        .fill(
          LinearGradient(
            colors: [tint.opacity(Self.fillOpacity), tint.opacity(0)], startPoint: .top,
            endPoint: .bottom))
      SparklinePath(samples: samples, closed: false)
        .stroke(tint, style: StrokeStyle(lineWidth: Self.lineWidth, lineJoin: .round))
    }
    .accessibilityHidden(true)
  }

  /// Where each sample sits in `rect`: evenly spread left to right, scaled so the largest
  /// sits at the top; a negative or non-finite sample counts as zero. One sample is a flat
  /// line across the width.
  nonisolated static func points(_ samples: [Double], in rect: CGRect) -> [CGPoint] {
    let values = samples.map { $0.isFinite ? max($0, 0) : 0 }
    guard let peak = values.max() else { return [] }
    let span = rect.height - insets.top - insets.bottom
    let scale = peak > 0 ? span / peak : 0
    let bottom = rect.maxY - insets.bottom
    guard values.count > 1 else {
      let y = bottom - values[0] * scale
      return [CGPoint(x: rect.minX, y: y), CGPoint(x: rect.maxX, y: y)]
    }
    let step = rect.width / CGFloat(values.count - 1)
    return values.enumerated().map { index, value in
      CGPoint(x: rect.minX + CGFloat(index) * step, y: bottom - value * scale)
    }
  }
}

/// The series as a line, or closed down to the frame's bottom edge for the fill.
private struct SparklinePath: Shape {
  let samples: [Double]
  let closed: Bool

  func path(in rect: CGRect) -> Path {
    let points = Sparkline.points(samples, in: rect)
    guard let first = points.first, let last = points.last else { return Path() }
    return Path { path in
      if closed {
        path.move(to: CGPoint(x: first.x, y: rect.maxY))
        // Not `addLines`: it starts a new subpath, which would drop the bottom-left corner.
        for point in points { path.addLine(to: point) }
        path.addLine(to: CGPoint(x: last.x, y: rect.maxY))
        path.closeSubpath()
      } else {
        path.addLines(points)
      }
    }
  }
}

#Preview("empty") { PreviewMatrix { SparklineSample(PreviewState.series[0].samples) } }
#Preview("one sample") { PreviewMatrix { SparklineSample(PreviewState.series[1].samples) } }
#Preview("full") { PreviewMatrix { SparklineSample(PreviewState.series[2].samples) } }
#Preview("sent tint") {
  PreviewMatrix { SparklineSample(PreviewState.series[2].samples, tint: Color(.meterSent)) }
}
