// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// DS§8's text contrast on glass (T37.21.11, A112): each text pair T37.21.11 and T37.22.11 fixed,
// measured on the glass laid over the window fill — the worst case cox can predict, since the
// wallpaper behind the window is unknown — in every material and both appearances. Separate from
// the snapshots because a render shows the snapshot wallpaper, and this checks the rule itself.

import AppKit
import Testing

@testable import CoxUI

@MainActor
@Suite struct ContrastTests {
  /// DS§8's floor for text.
  static let textRatio = 4.5

  /// A colour under the text: a pane's tint, drawn at the material's window opacity as
  /// `.glassPane` draws it, or a fill drawn at its own alpha.
  enum Layer {
    case pane(ColorResource)
    case fill(ColorResource)
  }

  struct Pair {
    let name: String
    let text: ColorResource
    /// Bottom up, over the opaque window fill.
    let layers: [Layer]
  }

  static let pairs = [
    Pair(
      name: "section header on the sidebar", text: .textSecondary,
      layers: [.pane(.surfaceSidebar)]),
    Pair(
      name: "section header on the inspector", text: .textSecondary,
      layers: [.pane(.surfaceWindow)]),
    Pair(
      name: "filter prompt in its well", text: .textPlaceholder,
      layers: [.pane(.surfaceSidebar), .fill(.fillPrimary)]),
    Pair(name: "Stop's key cap on its face", text: .surfaceWindow, layers: [.fill(.textPrimary)]),
    // A115: the selected session row's own fill, under its title and its subtitle and cost.
    Pair(
      name: "selected session row's title", text: .textPrimary,
      layers: [.pane(.surfaceSidebar), .fill(.accentSelected)]),
    Pair(
      name: "selected session row's subtitle", text: .textSecondary,
      layers: [.pane(.surfaceSidebar), .fill(.accentSelected)]),
  ]

  @Test func fixedTextPairsHoldDS8OnGlassOverTheWindowFill() throws {
    for dark in [false, true] {
      for material in GlassMaterial.allCases {
        let opacity = Appearance(material: material).backgroundOpacity(.chrome)
        for pair in Self.pairs {
          let ratio = try Self.ratio(pair, opacity: opacity, dark: dark)
          #expect(
            ratio >= Self.textRatio,
            "\(pair.name), \(material), \(dark ? "dark" : "light"): \(ratio):1")
        }
      }
    }
  }

  @Test func blackOnWhiteIsTwentyOneToOne() {
    let ratio = RGBA(red: 0, green: 0, blue: 0).contrast(with: RGBA(red: 1, green: 1, blue: 1))
    #expect(abs(ratio - 21) < 0.001)
  }

  private static func ratio(_ pair: Pair, opacity: Double, dark: Bool) throws -> Double {
    var under = try RGBA(.surfaceWindow, dark: dark)
    for layer in pair.layers {
      under =
        switch layer {
        case .pane(let surface): try RGBA(surface, dark: dark).over(under, opacity: opacity)
        case .fill(let fill): try RGBA(fill, dark: dark).over(under)
        }
    }
    return try RGBA(pair.text, dark: dark).over(under).contrast(with: under)
  }
}

/// An sRGB colour; compositing works on the encoded values, as `high-contrast.mjs` does.
private struct RGBA {
  var red: Double
  var green: Double
  var blue: Double
  var alpha = 1.0

  /// The asset colour as `appearance` resolves it.
  init(_ resource: ColorResource, dark: Bool) throws {
    let appearance = try #require(NSAppearance(named: dark ? .darkAqua : .aqua))
    var resolved: NSColor?
    appearance.performAsCurrentDrawingAppearance {
      resolved = NSColor(resource: resource).usingColorSpace(.sRGB)
    }
    let color = try #require(resolved)
    (red, green, blue, alpha) = (
      color.redComponent, color.greenComponent, color.blueComponent, color.alphaComponent
    )
  }

  init(red: Double, green: Double, blue: Double) {
    (self.red, self.green, self.blue) = (red, green, blue)
  }

  /// This colour at `opacity` of its alpha over the opaque `under`.
  func over(_ under: RGBA, opacity: Double = 1) -> RGBA {
    let share = alpha * opacity
    let mix = { (top: Double, bottom: Double) in top * share + bottom * (1 - share) }
    return RGBA(
      red: mix(red, under.red), green: mix(green, under.green), blue: mix(blue, under.blue))
  }

  /// WCAG 2's relative luminance.
  var luminance: Double {
    let linear = { (value: Double) in
      value <= 0.04045 ? value / 12.92 : pow((value + 0.055) / 1.055, 2.4)
    }
    return 0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
  }

  func contrast(with other: RGBA) -> Double {
    let (high, low) = (max(luminance, other.luminance), min(luminance, other.luminance))
    return (high + 0.05) / (low + 0.05)
  }
}
