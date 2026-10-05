// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Foundations' check (T37.19): one snapshot per modifier × light/dark × Solid/Frosted, the
// Reduce Transparency and Increase Contrast overrides, and the appearance arithmetic the
// modifiers share.

import AppKit
import SwiftUI
import Testing

@testable import CoxUI

/// Every material in both schemes, for the Increase Contrast renders (T37.19.6).
private let everyMaterial = [ColorScheme.light, .dark].flatMap { scheme in
  GlassMaterial.allCases.map { Variant(scheme: scheme, material: $0) }
}

@MainActor
@Suite struct FoundationsSnapshotTests {
  @Test(arguments: Variant.all) func elevation(_ variant: Variant) throws {
    try check(ElevationSample(), variant)
  }

  @Test(arguments: Variant.all) func glassPane(_ variant: Variant) throws {
    try check(GlassPaneSample(), variant)
  }

  @Test(arguments: Variant.all) func specular(_ variant: Variant) throws {
    try check(SpecularSample(), variant)
  }

  @Test(arguments: Variant.all) func hairline(_ variant: Variant) throws {
    try check(HairlineSample(), variant)
  }

  @Test(arguments: Variant.all) func insetWell(_ variant: Variant) throws {
    try check(InsetWellSample(), variant)
  }

  @Test(arguments: Variant.all) func textStyle(_ variant: Variant) throws {
    try check(TextStyleSample(), variant)
  }

  /// Increase Contrast (A100): no sweep, and glass keeps a quarter of its transparency.
  @Test(arguments: everyMaterial) func glassPaneIncreasedContrast(_ variant: Variant) throws {
    try check(GlassPaneSample().environment(\._colorSchemeContrast, .increased), variant)
  }

  /// `dark_highlight = subtle` (A109) at each scope; dark only, since light ignores the setting.
  @Test(arguments: HighlightScope.allCases, [GlassMaterial.solid, .frosted])
  func elevationSubtleDarkHighlight(_ scope: HighlightScope, _ material: GlassMaterial) throws {
    let variant = Variant(scheme: .dark, material: material)
    let appearance = Appearance(material: material, darkHighlight: .subtle, highlightScope: scope)
    try check(
      ElevationSample().environment(\.coxAppearance, appearance), variant,
      named: "\(scope)-\(variant.name)")
  }

  @Test func reduceTransparencyRendersSolid() throws {
    for scheme in [ColorScheme.light, .dark] {
      let forced = try SnapshotHost(
        GlassPaneSample(), Variant(scheme: scheme, material: .frosted), reduceTransparency: true
      ).bitmap()
      let solid = try SnapshotHost(GlassPaneSample(), Variant(scheme: scheme, material: .solid))
        .bitmap()
      #expect(forced.tiffRepresentation == solid.tiffRepresentation)
    }
  }

  private func check(
    _ sample: some View, _ variant: Variant, named name: String? = nil, test: String = #function
  ) throws {
    try assertCoxSnapshot(sample, variant, named: name ?? variant.name, testName: test)
  }
}

@Suite struct AppearanceTests {
  @Test func reduceTransparencyForcesSolidAndOpaque() {
    let forced = Appearance(material: .glossy).effective(reduceTransparency: true)
    #expect(forced.material == .solid)
    #expect(forced.backgroundOpacity(.chrome) == MaterialToken.solidWindowOpacity)
    #expect(forced.specular == MaterialToken.solidSpecular)
  }

  @Test func readableSurfacesNeverDropBelowTheFloor() {
    let clear = Appearance(material: .frosted, windowOpacity: 0)
    #expect(clear.backgroundOpacity(.chrome) == 0)
    #expect(clear.backgroundOpacity(.readable) == MaterialToken.readableFloorWindowOpacity)
  }

  @Test func increaseContrastDropsTheSweepAndKeepsAQuarterOfTheTransparency() {
    let glass = Appearance(material: .glossy, windowOpacity: 0)
      .effective(reduceTransparency: false, increaseContrast: true)
    #expect(glass.material == .glossy)
    #expect(glass.specular == MaterialToken.solidSpecular)
    #expect(glass.backgroundOpacity(.chrome) == 1 - MaterialToken.highContrastGlassKeep)
    #expect(
      glass.backgroundOpacity(.readable)
        == 1 - (1 - MaterialToken.readableFloorWindowOpacity) * MaterialToken.highContrastGlassKeep)
  }

  /// T51.23: a tint carries its own alpha, and its High Contrast value is in the palette, so
  /// neither the window opacity nor Increase Contrast scales it again.
  @Test func aTintIsDrawnAsItsTokenHasIt() {
    let clear = Appearance(material: .frosted, windowOpacity: 0)
    #expect(clear.backgroundOpacity(.tint) == 1)
    let contrast = clear.effective(reduceTransparency: false, increaseContrast: true)
    #expect(contrast.backgroundOpacity(.tint) == 1)
  }

  @Test func reduceTransparencyStillWinsOverIncreaseContrast() {
    let forced = Appearance(material: .frosted)
      .effective(reduceTransparency: true, increaseContrast: true)
    #expect(forced.material == .solid)
    #expect(forced.backgroundOpacity(.chrome) == MaterialToken.solidWindowOpacity)
    #expect(forced.backgroundOpacity(.readable) == MaterialToken.solidWindowOpacity)
  }

  @Test func standardContrastLeavesTheMaterialAlone() {
    let user = Appearance(material: .glossy)
    #expect(user.effective(reduceTransparency: false, increaseContrast: false) == user)
    #expect(user.specular == MaterialToken.glossySpecular)
    #expect(user.backgroundOpacity(.chrome) == MaterialToken.glossyWindowOpacity)
  }

  @Test func lightModeKeepsTheWholeHighlightWhateverTheSetting() {
    let light = Appearance(darkHighlight: .none, highlightScope: .all)
      .effective(reduceTransparency: false, colorScheme: .light)
    #expect([ElevationToken.e1, .e2, .e3, .e4].allSatisfy { light.highlightStrength($0) == 1 })
  }

  @Test func darkModeDropsTheControlHighlightByDefault() {
    let dark = Appearance().effective(reduceTransparency: false, colorScheme: .dark)
    #expect(dark.highlightStrength(.e1) == MaterialToken.darkHighlightNone)
    #expect(dark.highlightStrength(.e2) == 1)
  }

  /// T51.22: a lifted pane's top edge is `glass.highlight` as each scheme's token has it, not the
  /// light 0.95 in dark too.
  @MainActor @Test func theTopEdgeHighlightIsTheGlassHighlightTokenInEachScheme() throws {
    func topEdge(_ scheme: ColorScheme) throws -> Double {
      let drawn = Appearance().effective(reduceTransparency: false, colorScheme: scheme)
      let layer = try #require(ElevationToken.e2.layers(at: drawn).first { $0.inset && $0.y > 0 })
      return Double(layer.color.resolve(in: EnvironmentValues()).opacity)
    }
    let light = try topEdge(.light)
    let dark = try topEdge(.dark)
    #expect(abs(light - Appearance.glassHighlightAlpha(.aqua)) < 0.005)
    #expect(abs(dark - Appearance.glassHighlightAlpha(.darkAqua)) < 0.005)
    #expect(dark < light)
  }

  @Test func theSubtleHighlightReachesEveryLiftedLevelOnlyAtScopeAll() {
    func dark(_ scope: HighlightScope) -> Appearance {
      Appearance(darkHighlight: .subtle, highlightScope: scope)
        .effective(reduceTransparency: false, colorScheme: .dark)
    }
    #expect(dark(.controls).highlightStrength(.e1) == MaterialToken.darkHighlightSubtle)
    #expect(dark(.controls).highlightStrength(.e2) == 1)
    #expect(dark(.all).highlightStrength(.e2) == MaterialToken.darkHighlightSubtle)
    #expect(dark(.all).highlightStrength(.e4) == MaterialToken.darkHighlightSubtle)
  }

  @Test func windowOpacityDefaultsToTheMaterialToken() {
    #expect(Appearance(material: .glossy).windowOpacity == MaterialToken.glossyWindowOpacity)
    #expect(Appearance(material: .frosted).windowOpacity == MaterialToken.frostedWindowOpacity)
  }
}

private struct ElevationSample: View {
  let levels: [ElevationToken] = [.e0, .e1, .e2, .e3, .e4, .e5]

  var body: some View {
    HStack(spacing: Space.xxl) {
      ForEach(levels.indices, id: \.self) { index in
        RoundedRectangle(cornerRadius: Radius.l, style: .continuous)
          .fill(Color(.surfaceCapsule))
          .frame(width: Size.toolbarHeight, height: Size.toolbarHeight)
          .elevation(levels[index], cornerRadius: Radius.l)
      }
    }
    .padding(Space.huge)
    .background(Color(.surfaceWindow))
  }
}

private struct GlassPaneSample: View {
  var body: some View {
    HStack(spacing: Size.paneGap) {
      Text("Chrome pane")
        .frame(width: Size.popoverWidth / 2, height: Size.toolbarHeight * 2)
        .glassPane(RoundedRectangle(cornerRadius: Radius.pane, style: .continuous))
      Text("Readable pane")
        .frame(width: Size.popoverWidth / 2, height: Size.toolbarHeight * 2)
        .glassPane(
          RoundedRectangle(cornerRadius: Radius.pane, style: .continuous),
          surface: Color(.surfacePopover), role: .readable)
    }
    .textStyle(.body)
    .foregroundStyle(Color(.textPrimary))
  }
}

private struct SpecularSample: View {
  var body: some View {
    // The sweep lies under the content, so a surface behind it shows it.
    Color.clear
      .frame(width: Size.popoverWidth, height: Size.toolbarHeight * 2)
      .specular(MaterialToken.glossySpecular, in: .rect(cornerRadius: Radius.pane))
      .background {
        RoundedRectangle(cornerRadius: Radius.pane, style: .continuous)
          .fill(Color(.fillSecondary))
      }
  }
}

private struct HairlineSample: View {
  var body: some View {
    HStack(spacing: Space.xxl) {
      Color(.surfaceWindow)
        .frame(width: Size.toolbarHeight * 2, height: Size.toolbarHeight)
        .hairline([.top, .bottom])
      Color(.surfaceWindow)
        .frame(width: Size.toolbarHeight * 2, height: Size.toolbarHeight)
        .clipShape(.capsule)
        .hairline(in: .capsule)
    }
  }
}

private struct InsetWellSample: View {
  var body: some View {
    VStack(alignment: .leading, spacing: Space.xs) {
      Text("$ cargo test")
      Text("test result: ok").foregroundStyle(Color(.textTerminalOk))
    }
    .textStyle(.monoTerminal)
    .foregroundStyle(Color(.textTerminal))
    .padding(Space.ml)
    .frame(width: Size.popoverWidth, alignment: .leading)
    .insetWell()
  }
}

private struct TextStyleSample: View {
  let tokens: [FontToken] = [.titleWindow, .transcript, .control, .label, .metric, .monoCode]

  var body: some View {
    VStack(alignment: .leading, spacing: Space.xs) {
      ForEach(tokens.indices, id: \.self) { index in
        Text("Tokens 0123456789").textStyle(tokens[index])
      }
    }
    .foregroundStyle(Color(.textPrimary))
    .padding(Space.l)
    .background(Color(.surfaceWindow))
  }
}
