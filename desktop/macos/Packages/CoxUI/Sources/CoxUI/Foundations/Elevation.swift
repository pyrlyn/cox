// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `.elevation(_:)` (DS§3.4, DS§6.1): the only place a shadow is drawn, so the Depth setting
// scales every lifted thing at once. Drop shadows sit behind the view; inset layers are the
// top-edge highlight drawn inside the view's shape.

import AppKit
import SwiftUI

extension View {
  /// Lifts the view to `level`, its highlight following a rounded shape of `cornerRadius`.
  func elevation(_ level: ElevationToken, cornerRadius: CGFloat = 0) -> some View {
    modifier(
      Elevation(
        level: level,
        shape: RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)))
  }
}

private struct Elevation: ViewModifier {
  let level: ElevationToken
  let shape: RoundedRectangle
  @EffectiveAppearance private var appearance

  func body(content: Content) -> some View {
    let layers = level.layers(at: appearance)
    return
      content
      .modifier(DropShadows(layers: layers.filter { !$0.inset }))
      .overlay { highlights(layers.filter(\.inset)) }
  }

  private func highlights(_ layers: [ShadowLayer]) -> some View {
    ZStack {
      ForEach(Array(layers.enumerated()), id: \.offset) { _, layer in
        shape.subtracting(
          shape.inset(by: layer.spread).offset(x: layer.x, y: layer.y)
        )
        .fill(layer.color)
      }
    }
    .allowsHitTesting(false)
  }
}

extension ElevationToken {
  /// The level's layers at `appearance`'s Depth (DS§3.4): each layer's opacity, and a drop
  /// shadow's offset down, scaled by Depth; the window's level ignores it. The highlight takes
  /// `glass.highlight`'s strength and the dark-mode share (DS§3.5, A109). Public for the
  /// transcript's AppKit bubble (T37.23.9), so it lifts as `.elevation` does.
  @MainActor
  public func layers(at appearance: Appearance) -> [ShadowLayer] {
    let scale = self == .e5 ? 1 : appearance.depth
    let highlight =
      scale * appearance.highlightStrength(self) * appearance.glassHighlightShare
    return layers.map {
      ShadowLayer(
        color: $0.color.opacity($0.inset ? highlight : scale), x: $0.x,
        y: $0.inset ? $0.y : $0.y * scale,
        blur: $0.blur, spread: $0.spread, inset: $0.inset)
    }
  }
}

extension Appearance {
  /// `glass.highlight` where this appearance draws, as a share of its light value (DS§3.5).
  /// The elevation tokens hold the light highlight (mockup 28's); dark glass draws it at its
  /// own, lower strength (mockups 31, 32), and High Contrast at its own. Read from the colour
  /// asset, so the value lives only in the token files.
  @MainActor
  var glassHighlightShare: Double {
    let light = Self.glassHighlightAlpha(.aqua)
    guard light > 0 else { return 0 }
    let drawn: NSAppearance.Name =
      switch (isDark, increaseContrast) {
      case (false, false): .aqua
      case (true, false): .darkAqua
      case (false, true): .accessibilityHighContrastAqua
      case (true, true): .accessibilityHighContrastDarkAqua
      }
    return Self.glassHighlightAlpha(drawn) / light
  }

  /// `glass.highlight`'s alpha as the colour asset resolves it under `name`.
  @MainActor
  static func glassHighlightAlpha(_ name: NSAppearance.Name) -> Double {
    var alpha = 0.0
    NSAppearance(named: name)?.performAsCurrentDrawingAppearance {
      alpha = Double(NSColor(resource: .glassHighlight).usingColorSpace(.sRGB)?.alphaComponent ?? 0)
    }
    return alpha
  }
}

/// Stacks one `.shadow` per layer; CSS blur is twice SwiftUI's radius.
private struct DropShadows: ViewModifier {
  let layers: [ShadowLayer]

  func body(content: Content) -> some View {
    layers.reduce(AnyView(content)) { view, layer in
      AnyView(view.shadow(color: layer.color, radius: layer.blur / 2, x: layer.x, y: layer.y))
    }
  }
}
