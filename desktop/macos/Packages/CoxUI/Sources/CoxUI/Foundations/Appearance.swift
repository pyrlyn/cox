// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The appearance every Foundation reads (DS§3.4–3.6): material, window opacity, Depth, text
// size and the dark highlight (A109), plus the one place the system's Reduce Transparency,
// Increase Contrast and Reduce Motion override them (DS§1.6, DS§8). Separate so no modifier
// resolves a setting on its own. CoxModel fills `coxAppearance` from `[desktop.appearance]`;
// CoxUI never reads config. The modifiers in Foundations are internal: only CoxUI's own
// components style a view (DS§5).

import SwiftUI

/// The window material the user picked (DS§3.5).
public enum GlassMaterial: String, Sendable, CaseIterable {
  case solid, frosted, glossy
}

/// The top-edge highlight lifted things draw in dark mode (A109), as `dark_highlight` spells it.
public enum DarkHighlight: String, Sendable, CaseIterable {
  /// None: the dark mockup's look.
  case none
  /// `MaterialToken.darkHighlightSubtle` of the light highlight's strength.
  case subtle
}

/// The elevation levels `DarkHighlight` applies to (A109); the others keep the light highlight.
public enum HighlightScope: String, Sendable, CaseIterable {
  /// Controls only: e1.
  case controls
  /// Every lifted level, e1–e4.
  case all
}

/// What a glass surface carries, which decides whether it may go below the readable floor.
enum SurfaceRole: Sendable {
  /// Window and pane backgrounds: they follow the transparency slider.
  case chrome
  /// Messages, code, diffs, terminal, popovers, the composer: never below the floor.
  case readable
  /// A glass token that carries its own alpha (`glass.fill`, the panes over the window): drawn
  /// as the token has it, since its High Contrast value is already in the palette (A89).
  case tint
}

/// The `[desktop.appearance]` values the Foundations draw with.
public struct Appearance: Sendable, Equatable {
  public var material: GlassMaterial
  /// Window and pane background opacity, 0…1 (the transparency slider).
  public var windowOpacity: Double
  /// Depth, 0 (Flat) … 1 (3D): scales every elevation but `e5` (DS§3.4).
  public var depth: Double
  /// Text size, 1 = 100 % (DS§3.2).
  public var textScale: Double
  /// The highlight dark mode draws, and on which levels (A109).
  public var darkHighlight: DarkHighlight
  public var highlightScope: HighlightScope
  /// The system asked for more contrast; only `effective` sets it, the config never does.
  private(set) var increaseContrast = false
  /// The view is drawn dark; only `effective` sets it, from the environment's colour scheme.
  private(set) var isDark = false

  /// `windowOpacity` defaults to the material's token.
  public init(
    material: GlassMaterial = .frosted, windowOpacity: Double? = nil, depth: Double = 1,
    textScale: Double = 1, darkHighlight: DarkHighlight = .none,
    highlightScope: HighlightScope = .controls
  ) {
    self.material = material
    self.windowOpacity = windowOpacity ?? Self.defaultOpacity(material)
    self.depth = depth
    self.textScale = textScale
    self.darkHighlight = darkHighlight
    self.highlightScope = highlightScope
  }

  /// What a view draws: Reduce Transparency forces Solid (DS§1.6); Increase Contrast drops the
  /// specular sweep and makes glass more opaque (A89, A100), and Solid stays Solid under both.
  /// `colorScheme` picks the dark highlight (A109).
  public func effective(
    reduceTransparency: Bool, increaseContrast: Bool = false, colorScheme: ColorScheme = .light
  ) -> Appearance {
    var drawn = self
    drawn.increaseContrast = increaseContrast
    drawn.isDark = colorScheme == .dark
    guard reduceTransparency, material != .solid else { return drawn }
    drawn.material = .solid
    drawn.windowOpacity = MaterialToken.solidWindowOpacity
    return drawn
  }

  /// Background opacity of a surface: Solid is opaque, readable surfaces hold the floor, and
  /// under Increase Contrast glass keeps `highContrastGlassKeep` of its transparency — the
  /// rule the High Contrast palette applies to its glass colours (A89).
  func backgroundOpacity(_ role: SurfaceRole) -> Double {
    let opacity =
      switch (material, role) {
      case (.solid, _), (_, .tint): MaterialToken.solidWindowOpacity
      case (_, .chrome): windowOpacity
      case (_, .readable): max(windowOpacity, MaterialToken.readableFloorWindowOpacity)
      }
    guard increaseContrast, role != .tint else { return opacity }
    return 1 - (1 - opacity) * MaterialToken.highContrastGlassKeep
  }

  /// Strength of the diagonal highlight for the material (DS§3.5); none under Increase Contrast.
  var specular: Double {
    if increaseContrast { return MaterialToken.solidSpecular }
    return switch material {
    case .solid: MaterialToken.solidSpecular
    case .frosted: MaterialToken.frostedSpecular
    case .glossy: MaterialToken.glossySpecular
    }
  }

  /// The share of `level`'s inset highlight to draw: all of it in light, and in dark the
  /// `darkHighlight` share on the levels `highlightScope` names (A109).
  func highlightStrength(_ level: ElevationToken) -> Double {
    guard isDark, highlightScope == .all || level == .e1 else { return 1 }
    return switch darkHighlight {
    case .none: MaterialToken.darkHighlightNone
    case .subtle: MaterialToken.darkHighlightSubtle
    }
  }

  private static func defaultOpacity(_ material: GlassMaterial) -> Double {
    switch material {
    case .solid: MaterialToken.solidWindowOpacity
    case .frosted: MaterialToken.frostedWindowOpacity
    case .glossy: MaterialToken.glossyWindowOpacity
    }
  }
}

extension EnvironmentValues {
  /// The user's appearance, before the system overrides it; read it through `EffectiveAppearance`.
  @Entry public var coxAppearance = Appearance()
}

/// The appearance with Reduce Transparency and Increase Contrast applied — the only way a
/// Foundation reads it.
@propertyWrapper
struct EffectiveAppearance: DynamicProperty {
  @Environment(\.coxAppearance) private var appearance
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
  @Environment(\.colorSchemeContrast) private var contrast
  @Environment(\.colorScheme) private var colorScheme

  var wrappedValue: Appearance {
    appearance.effective(
      reduceTransparency: reduceTransparency, increaseContrast: contrast == .increased,
      colorScheme: colorScheme)
  }
}

extension Animation {
  /// A token animation: `Motion.duration*` on `Motion.easing*` (DS§3.6).
  static func cox(
    _ duration: TimeInterval, curve: UnitCurve = Motion.easingStandard
  ) -> Animation {
    .timingCurve(curve, duration: duration)
  }
}

extension View {
  /// Inserts and removes with `movement`, or with a cross-fade under Reduce Motion (DS§3.6).
  func coxTransition(_ movement: AnyTransition) -> some View {
    modifier(CoxTransition(movement: movement))
  }
}

private struct CoxTransition: ViewModifier {
  let movement: AnyTransition
  @Environment(\.accessibilityReduceMotion) private var reduceMotion

  func body(content: Content) -> some View {
    content.transition(reduceMotion ? .opacity : movement)
  }
}

extension View {
  /// Turns the view a full circle every `period`, for as long as it is on screen — a busy
  /// indicator — or holds it still under Reduce Motion, where a turn has no cross-fade to
  /// become (DS§3.6).
  func coxSpin(period: TimeInterval) -> some View {
    modifier(CoxSpin(period: period))
  }
}

private struct CoxSpin: ViewModifier {
  let period: TimeInterval
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  /// The angle counts from the view's first appearance, so every spinner starts upright.
  @State private var start = Date.now

  func body(content: Content) -> some View {
    if reduceMotion {
      content
    } else {
      TimelineView(.animation) { timeline in
        content.rotationEffect(angle(at: timeline.date))
      }
    }
  }

  private func angle(at date: Date) -> Angle {
    let turn = date.timeIntervalSince(start) / period
    return .degrees((turn - turn.rounded(.down)) * 360)
  }
}

extension View {
  /// One view that moves between places — a selection pill, a knob: the copy that appears
  /// slides from the frame of the copy that goes, sharing `id` in `namespace`, or the two
  /// cross-fade under Reduce Motion (DS§3.6).
  func coxMatchedGeometry(id: some Hashable, in namespace: Namespace.ID) -> some View {
    modifier(CoxMatchedGeometry(id: AnyHashable(id), namespace: namespace))
  }
}

private struct CoxMatchedGeometry: ViewModifier {
  let id: AnyHashable
  let namespace: Namespace.ID
  @Environment(\.accessibilityReduceMotion) private var reduceMotion

  /// Both copies fade: while they fly together the hand-over is invisible, and it keeps the
  /// copy that goes on screen long enough for the one that appears to match its frame.
  /// Under Reduce Motion nothing is matched, so the fade is all that is left.
  func body(content: Content) -> some View {
    content
      .matchedGeometryEffect(id: id, in: namespace, properties: reduceMotion ? [] : .frame)
      .transition(.opacity)
  }
}
