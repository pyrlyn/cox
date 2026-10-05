// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// What sits on glass keeps its token colour (T37.22.10): a `text.primary` swatch in a pane's top
// leading corner, where the specular sweep is brightest, reads the same on Frosted and Glossy
// as on Solid, which draws no sweep. Separate from the snapshots because a snapshot passes with
// the sweep drawn over the text; this measures the one pixel that shows it.

import AppKit
import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct GlassContentTests {
  /// A pane with a swatch at the corner the sweep starts from.
  private struct Swatch: View {
    let frosts: Bool
    var body: some View {
      ZStack(alignment: .topLeading) {
        Color.clear
        Color(.textPrimary).frame(width: Size.capsuleHeight, height: Size.capsuleHeight)
      }
      .frame(width: Size.popoverWidth, height: Size.popoverWidth)
      .glassPane(Rectangle(), frosts: frosts)
    }
  }

  /// The swatch's centre as drawn in `material`, in the host's 2× bitmap.
  private func swatch(_ material: GlassMaterial, frosts: Bool) throws -> NSColor {
    let variant = Variant(scheme: .light, material: material)
    let bitmap = try SnapshotHost(Swatch(frosts: frosts), variant).bitmap()
    // The sample sits `Space.xxl` in from the host's edge; the swatch is `capsuleHeight` square.
    let centre = Int((Space.xxl + Size.capsuleHeight / 2) * 2)
    return try #require(bitmap.colorAt(x: centre, y: centre)?.usingColorSpace(.deviceRGB))
  }

  @Test(arguments: [GlassMaterial.frosted, .glossy], [false, true])
  func contentKeepsItsTokenColourUnderTheSweep(_ material: GlassMaterial, _ frosts: Bool) throws {
    let solid = try swatch(.solid, frosts: frosts)
    let glass = try swatch(material, frosts: frosts)
    let drift = max(
      abs(glass.redComponent - solid.redComponent),
      abs(glass.greenComponent - solid.greenComponent),
      abs(glass.blueComponent - solid.blueComponent))
    #expect(drift < 0.02, "the swatch drifted \(drift) from its token: \(glass) vs \(solid)")
  }
}
