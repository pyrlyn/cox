// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `CoxSegmented`'s check (T37.19.3): a snapshot per selection × light/dark × Solid/Frosted, and
// the selection sliding between segments, or cross-fading under Reduce Motion.

import AppKit
import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct SegmentedTests {
  @Test(arguments: Variant.all) func selection(_ variant: Variant) throws {
    for mode in SegmentedSample.modes {
      try assertCoxSnapshot(
        SegmentedSample(selection: mode), variant, named: "\(mode).\(variant.name)")
    }
  }

  /// Half way the pill sits on the middle segment and has left both ends.
  @Test func selectionSlidesThroughTheMiddleSegment() throws {
    let cover = try halfWay(reduceMotion: false)
    #expect(cover[1] > 0.4, "\(cover)")
    #expect(cover[0] < 0.1 && cover[2] < 0.1, "\(cover)")
  }

  /// Half way both ends hold part of the pill and the middle none of it.
  @Test func reduceMotionCrossFadesTheSelection() throws {
    let cover = try halfWay(reduceMotion: true)
    #expect(cover[1] < 0.1, "\(cover)")
    #expect([cover[0], cover[2]].allSatisfy { $0 > 0.1 && $0 < 0.9 }, "\(cover)")
  }

  /// Moves the selection from the first mode to the last under an animation held at half its
  /// progress, and returns how much of the pill covers each segment (0 bare … 1 selected) in
  /// the first frame where it has left the first segment. The held animation, not the time a
  /// frame took, decides what that frame shows (T37.21: the sampled version missed its
  /// mid-fade frame under load).
  private func halfWay(reduceMotion: Bool) throws -> [Double] {
    let (first, last) = (SegmentedSample.modes[0], SegmentedSample.modes[2])
    let cell = Variant(scheme: .light, material: .solid)
    let choice = Choice(selection: first)
    let control = SnapshotHost(LiveSample(choice: choice), cell, reduceMotion: reduceMotion)
    let probe = try PillProbe(
      before: control.bitmap(),
      after: SnapshotHost(SegmentedSample(selection: last), cell).bitmap())
    // `disablesAnimations` keeps the control's own implicit animation from replacing this one.
    var held = Transaction(animation: Animation(HalfWay()))
    held.disablesAnimations = true
    withTransaction(held) { choice.selection = last }
    return probe.cover(try control.bitmap { probe.cover($0)[0] < 0.9 })
  }
}

/// An animation that stays at half its progress for as long as it runs.
private struct HalfWay: CustomAnimation {
  func animate<V: VectorArithmetic>(
    value: V, time: TimeInterval, context: inout AnimationContext<V>
  ) -> V? {
    value.scaled(by: 0.5)
  }
}

/// Reads the pill's cover in the middle of each segment, just above its title.
private struct PillProbe {
  let points: [(x: Int, y: Int)]
  let pill: NSColor
  let bare: [NSColor]

  /// `before` has the pill on the first segment, `after` on the last.
  init(before: NSBitmapImageRep, after: NSBitmapImageRep) throws {
    // Pixels at 2×; the control sits inside the backdrop's `Space.xxl` margin.
    let margin = Int(Space.xxl * 2)
    let control = before.pixelsWide - margin * 2
    let row = margin + Int((Space.xxs + Space.xs) * 2)
    let points = [1, 3, 5].map { (x: margin + control * $0 / 6, y: row) }
    self.points = points
    pill = try #require(before.colorAt(x: points[0].x, y: row))
    bare = try [after, before, before].enumerated().map { index, bitmap in
      try #require(bitmap.colorAt(x: points[index].x, y: row))
    }
  }

  func cover(_ frame: NSBitmapImageRep) -> [Double] {
    points.indices.map { index in
      let point = points[index]
      guard let color = frame.colorAt(x: point.x, y: point.y) else { return 0 }
      return Self.distance(color, bare[index]) / Self.distance(pill, bare[index])
    }
  }

  private static func distance(_ lhs: NSColor, _ rhs: NSColor) -> Double {
    abs(lhs.redComponent - rhs.redComponent) + abs(lhs.greenComponent - rhs.greenComponent)
      + abs(lhs.blueComponent - rhs.blueComponent)
  }
}

private struct SegmentedSample: View {
  static let modes = ["Ask", "Plan", "Auto"]
  let selection: String

  var body: some View {
    CoxSegmented("Mode", selection: .constant(selection), options: Self.modes) {
      Text(verbatim: $0)
    }
  }
}

@MainActor @Observable private final class Choice {
  var selection: String
  init(selection: String) { self.selection = selection }
}

/// The control bound to a model, so a test changes its selection inside a transaction.
private struct LiveSample: View {
  let choice: Choice

  var body: some View {
    CoxSegmented(
      "Mode", selection: Bindable(choice).selection, options: SegmentedSample.modes
    ) { Text(verbatim: $0) }
  }
}
