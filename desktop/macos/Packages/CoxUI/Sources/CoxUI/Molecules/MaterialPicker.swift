// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `MaterialPicker` (DS§6.3 row `MaterialPicker`, the mockup's `.mat`): the Appearance popover's
// choice of window glass — Frosted, Glossy or Solid — as three swatches, each a small pane drawn
// in its own material over a wallpaper and lifted at the user's Depth. Separate so the popover
// and Settings › Appearance offer the materials the same way. Not a `CoxSegmented`: a segment
// is a capsule-high label, and a swatch has to show the glass itself.

import SwiftUI

/// Three swatch tiles on the readable capsule glass at e1, rimmed by a hairline so they hold
/// their shape at Flat; the selected one wears an `accent` ring. VoiceOver sees a picker, as for
/// `CoxSegmented`.
struct MaterialPicker: View {
  @Binding var selection: GlassMaterial

  /// The mockup's order: the default first, the plainest last.
  nonisolated static let order: [GlassMaterial] = [.frosted, .glossy, .solid]

  var body: some View {
    HStack(spacing: Space.m) {
      ForEach(Self.order, id: \.self) { material in
        MaterialSwatch(material: material, isSelected: material == selection) {
          selection = material
        }
      }
    }
    .accessibilityRepresentation {
      Picker("Material", selection: $selection) {
        ForEach(Self.order, id: \.self) { Text($0.title).tag($0) }
      }
      .pickerStyle(.segmented)
    }
  }
}

extension GlassMaterial {
  /// The name a swatch shows.
  var title: String {
    switch self {
    case .frosted: "Frosted"
    case .glossy: "Glossy"
    case .solid: "Solid"
    }
  }
}

extension Appearance {
  /// What a swatch of `material` draws with: that material at its own default opacity, at this
  /// appearance's Depth, text size and dark highlight, so every swatch previews the lift the
  /// user set.
  func swatch(_ material: GlassMaterial) -> Appearance {
    Appearance(
      material: material, depth: depth, textScale: textScale, darkHighlight: darkHighlight,
      highlightScope: highlightScope)
  }
}

/// One tile: the preview over the material's name, concentric with its face.
private struct MaterialSwatch: View {
  let material: GlassMaterial
  let isSelected: Bool
  let select: () -> Void

  var body: some View {
    let face = RoundedRectangle(cornerRadius: Radius.xl, style: .continuous)
    Button(action: select) {
      VStack(spacing: Space.xs) {
        MaterialPreview(material: material)
        Text(material.title)
          .textStyle(.footnote)
          .fontWeight(.medium)
          .foregroundStyle(Color(.textPrimary))
      }
      .padding([.horizontal, .top], Space.xs)
      .padding(.bottom, Space.s)
      .frame(maxWidth: .infinity)
      .glassPane(face, surface: Color(.surfaceCapsule), role: .readable)
      .hairline(in: face)
      .elevation(.e1, cornerRadius: Radius.xl)
      .overlay {
        if isSelected {
          face.inset(by: -Space.xxs).strokeBorder(Color(.accent), lineWidth: Space.xxs)
        }
      }
      .contentShape(face)
    }
    .buttonStyle(.plain)
  }
}

/// A wallpaper with a pane of `material` on it, lifted to e2 at the user's Depth.
private struct MaterialPreview: View {
  let material: GlassMaterial
  @Environment(\.coxAppearance) private var appearance

  /// The mockup's `.mat .sw` height; no size token is near it.
  private static let height: CGFloat = 40

  var body: some View {
    let pane = RoundedRectangle(cornerRadius: Radius.xs, style: .continuous)
    ZStack {
      LinearGradient(
        colors: [Color(.accent), Color(.statusPlan), Color(.statusWarning)],
        startPoint: .topLeading, endPoint: .bottomTrailing)
      Color.clear
        .glassPane(pane)
        .elevation(.e2, cornerRadius: Radius.xs)
        .padding(Space.xs)
        .environment(\.coxAppearance, appearance.swatch(material))
    }
    .frame(height: Self.height)
    .clipShape(RoundedRectangle(cornerRadius: Radius.m, style: .continuous))
    .accessibilityHidden(true)
  }
}

#Preview("frosted") { PreviewMatrix { MaterialPickerSample(selection: .frosted) } }
#Preview("glossy") { PreviewMatrix { MaterialPickerSample(selection: .glossy) } }
#Preview("solid") { PreviewMatrix { MaterialPickerSample(selection: .solid) } }
#Preview("flat") { PreviewMatrix { MaterialPickerSample(selection: .frosted, depth: 0) } }
