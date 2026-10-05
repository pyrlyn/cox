// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `.specular(_:in:)` (DS§3.5, DS§6.1): the diagonal sweep and streak that make glass read as
// glass, the mockup's `.window:after`. Separate so the sweep's shape lives in one place; it
// draws nothing when the effective appearance has no specular — Solid, so Reduce Transparency
// removes it too, and Increase Contrast (A89). It lights the glass, never what sits on it: drawn
// over the content, its white corner washed every pane's text out (T37.22.10).

import SwiftUI

extension View {
  /// Backs the view with the highlight at `strength` (a `MaterialToken.*Specular`), clipped to
  /// `shape`: under the content and over any surface the caller adds after it, so text and
  /// icons keep their token colour at every point of the sweep.
  func specular(_ strength: Double, in shape: some Shape = Rectangle()) -> some View {
    modifier(Specular(strength: strength, shape: shape))
  }
}

private struct Specular<S: Shape>: ViewModifier {
  let strength: Double
  let shape: S
  @EffectiveAppearance private var appearance

  func body(content: Content) -> some View {
    // A background, not an overlay: a translucent light over the content would lighten it by
    // up to `strength`, most at the top leading corner where each pane's first lines sit.
    content.background {
      if appearance.specular > 0, strength > 0 {
        shape.fill(sweep).allowsHitTesting(false)
      }
    }
  }

  private var sweep: LinearGradient {
    LinearGradient(
      stops: Appearance.sweep(strength).map {
        .init(color: Color(.glassSpecular).opacity($0.opacity), location: $0.location)
      },
      startPoint: .topLeading,
      endPoint: .bottomTrailing
    )
  }
}

extension Appearance {
  /// The mockup's 118° gradient at `strength`: a bright corner, a clear middle and a thin
  /// streak, as opacities of `glass.specular` by location. Shared with the transcript's AppKit bubble
  /// (`sweepStops`), so both draw one sweep.
  static func sweep(_ strength: Double) -> [(opacity: Double, location: Double)] {
    [
      (strength, 0), (strength * 0.25, 0.18), (0, 0.30), (0, 0.64), (strength * 0.4, 0.66),
      (0, 0.72),
    ]
  }
}
