// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `Sparkline` and `StackedBar`'s check (T37.20.2, DS§6.2): snapshots of the empty, one-sample
// and full series and of each bar mix × light/dark × Solid/Frosted, the sparkline's scaling
// and the bar's clamping and spoken split.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct MeterAtomTests {
  @Test(arguments: Variant.all) func sparkline(_ variant: Variant) throws {
    for (name, samples) in PreviewState.series {
      try check(SparklineSample(samples), variant, name)
    }
    try check(
      SparklineSample(PreviewState.series[2].samples, tint: Color(.meterSent)), variant, "sent")
  }

  @Test(arguments: Variant.all) func stackedBar(_ variant: Variant) throws {
    for (name, segments) in PreviewState.bars {
      try check(StackedBarSample(segments), variant, name)
    }
  }

  @Test func sparklinePutsTheLargestSampleAtTheTopAndZeroAtTheBottom() {
    let points = Sparkline.points([0, 5, 10, -3, .infinity], in: Self.frame)
    #expect(points.map(\.x) == [0, 7.5, 15, 22.5, 30])
    #expect(points.map(\.y) == [12, 7, 2, 12, 12])
  }

  @Test func sparklineDrawsOneSampleAcrossTheWidthAndNoneAsNothing() {
    #expect(Sparkline.points([4], in: Self.frame).map(\.x) == [0, 30])
    #expect(Sparkline.points([4], in: Self.frame).map(\.y) == [2, 2])
    #expect(Sparkline.points([0], in: Self.frame).map(\.y) == [12, 12])
    #expect(Sparkline.points([], in: Self.frame).isEmpty)
  }

  @Test func stackedBarCutsWhatRunsPastTheEnd() {
    let bar = StackedBar([
      .init(kind: .system, fraction: -0.2), .init(kind: .tools, fraction: 0.75),
      .init(kind: .history, fraction: 0.5), .init(kind: .instructions, fraction: .nan),
      .init(kind: .instructions, fraction: 0.1),
    ])
    #expect(bar.segments.map(\.fraction) == [0, 0.75, 0.25, 0, 0])
  }

  @Test func stackedBarReadsEachPartAndItsShare() {
    #expect(
      StackedBar(PreviewState.bars[2].segments).label(Locale(identifier: "en_US"))
        == "system 3%, tools 5%, instruction files 6%, history 24%")
  }

  /// 30 × 13 pt: a 10 pt span between the 2 pt top and 1 pt bottom insets, 7.5 pt per step
  /// across four gaps.
  private static let frame: CGRect = {
    let (wide, tall): (CGFloat, CGFloat) = (30, 13)
    return CGRect(x: 0, y: 0, width: wide, height: tall)
  }()

  /// One image per look, named `<look>.<cell>`.
  private func check(
    _ atom: some View, _ variant: Variant, _ look: String, test: String = #function
  ) throws {
    try assertCoxSnapshot(
      PreviewPane { atom }, variant, named: "\(look).\(variant.name)", testName: test)
  }
}
