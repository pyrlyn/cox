// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `AppearancePopover` (DS§3.5, DS§6.4 row `AppearancePopover`, the mockup's `.appear`; mockup
// screens 28–29): the window's glass settings — material, transparency, blur or reflection,
// Depth and wallpaper tint — under the toolbar's paintbrush. Separate so the window shell
// shows them from one value and reports each change as an intent the app writes to
// `[desktop.appearance]`; the window redraws from the same value, so a change shows live. A
// control a config layer above the user file sets is disabled, and the note names the layer.

import SwiftUI

/// The popover on readable popover glass at e4. It holds no setting: every control shows
/// `state` and reports a change through `send`. Under Reduce Transparency the window is Solid
/// whatever is picked, so the glass controls are disabled and the note says why (DS§1.6).
public struct AppearancePopover: View {
  /// `[desktop.appearance]` as the core stored it, and its values as the core formats them.
  public struct State: Equatable, Sendable {
    public var material = GlassMaterial.frosted
    /// Window and pane background opacity, 0 (clear) … 1 (opaque); the slider shows its
    /// complement, transparency.
    public var opacity = MaterialToken.frostedWindowOpacity
    /// Blur in pt for Frosted, reflection for Glossy, within `blurRange` from the schema.
    public var blur = MaterialToken.frostedBlur
    public var blurRange: ClosedRange<Double> = MaterialToken.solidBlur...MaterialToken.frostedBlur
    /// Depth, 0 (Flat) … 1 (3D).
    public var depth = 1.0
    public var tint = true
    /// `58%`, `34 pt`, `High`.
    public var transparencyText = ""
    public var blurText = ""
    public var depthText = ""
    /// The controls a layer above the user file sets, each with that layer's name (`project`):
    /// an edit would not take effect, so they are disabled.
    public var locked: [Control: String] = [:]

    /// Frosted at its token values, nothing locked: what the window shows before the config loads.
    public init() {}

    /// The window these values draw, over `base`'s text size: what `coxAppearance` holds
    /// while the popover edits it.
    public func applied(to base: Appearance) -> Appearance {
      var window = base
      window.material = material
      window.windowOpacity = opacity
      window.depth = depth
      return window
    }

    /// Takes a change at once, so the window follows a slider while the core stores it; the
    /// texts stay until the core formats the stored value.
    public mutating func apply(_ change: Intent) {
      switch change {
      case .material(let value): material = value
      case .opacity(let value): opacity = value
      case .blur(let value): blur = value
      case .depth(let value): depth = value
      case .tint(let value): tint = value
      }
    }
  }

  /// One control's change, named as its `[desktop.appearance]` key.
  public enum Intent: Equatable, Sendable {
    case material(GlassMaterial)
    case opacity(Double)
    case blur(Double)
    case depth(Double)
    case tint(Bool)
  }

  /// The popover's controls, by the `[desktop.appearance]` key each one sets.
  public enum Control: String, CaseIterable, Sendable {
    case material, opacity, blur, depth, tint
  }

  let state: State
  let send: (Intent) -> Void
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency

  public var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.popover, style: .continuous)
    let isGlass = state.material != .solid
    VStack(alignment: .leading, spacing: Space.l) {
      HStack {
        Text("Appearance").textStyle(.body).fontWeight(.semibold)
          .foregroundStyle(Color(.textPrimary))
          .accessibilityAddTraits(.isHeader)
        Spacer(minLength: Space.m)
        KeyCap(ShellShortcut.appearance.glyphs)
      }
      VStack(spacing: Space.s) {
        SectionHeader("Material")
        MaterialPicker(selection: bind(\.material, Intent.material))
      }
      .disabled(reduceTransparency || isLocked(.material))
      Group {
        LabeledSlider(
          "Window transparency",
          value: transparency,
          valueText: state.transparencyText, ends: (low: "Opaque", high: "Clear")
        )
        .disabled(isLocked(.opacity))
        blurSlider.disabled(isLocked(.blur))
      }
      .disabled(reduceTransparency || !isGlass)
      LabeledSlider(
        "Depth", value: bind(\.depth, Intent.depth), valueText: state.depthText,
        ends: (low: "Flat", high: "3D")
      )
      .disabled(isLocked(.depth))
      LabeledToggle("Tint from wallpaper", isOn: bind(\.tint, Intent.tint))
        .disabled(reduceTransparency || isLocked(.tint))
      Text(note)
        .textStyle(.footnote)
        .foregroundStyle(
          Color(reduceTransparency || !state.locked.isEmpty ? .textSecondary : .textTertiary)
        )
        .fixedSize(horizontal: false, vertical: true)
    }
    .padding(.horizontal, Space.xl)
    .padding(.top, Space.popover)
    .padding(.bottom, Space.xl)
    .frame(width: Size.popoverWidth)
    .glassPane(shape, surface: Color(.surfacePopover), role: .readable)
    .hairline(in: shape)
    .elevation(.e4, cornerRadius: Radius.popover)
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Appearance")
  }

  /// Frosted blurs the wallpaper; Glossy reads the same value as reflection (T37.13).
  private var blurSlider: some View {
    let isGlossy = state.material == .glossy
    return LabeledSlider(
      isGlossy ? "Reflection" : "Frost (blur)", value: bind(\.blur, Intent.blur),
      in: state.blurRange, valueText: state.blurText,
      ends: isGlossy ? (low: "Matte", high: "Mirror") : (low: "Light", high: "Heavy"))
  }

  private var note: String {
    if reduceTransparency { return "Reduce transparency is on, so the window stays Solid." }
    // One sentence per layer, its controls in the popover's order.
    let layers = Set(state.locked.values).sorted()
    guard !layers.isEmpty else { return "Text panels stay readable at every setting." }
    return layers.map { layer in
      let names = Control.allCases.filter { state.locked[$0] == layer }.map(\.title)
      return "Set by the \(layer) layer: \(names.formatted(.list(type: .and)))."
    }
    .joined(separator: " ")
  }

  private func isLocked(_ control: Control) -> Bool { state.locked[control] != nil }

  /// The transparency slider: the complement of the opacity the config stores.
  var transparency: Binding<Double> {
    Binding(get: { 1 - state.opacity }, set: { send(.opacity(1 - $0)) })
  }

  /// A control's binding: it shows `state` and reports a change as `intent`.
  func bind<Value>(
    _ field: KeyPath<State, Value>, _ intent: @escaping (Value) -> Intent
  ) -> Binding<Value> {
    Binding(get: { state[keyPath: field] }, set: { send(intent($0)) })
  }
}

extension AppearancePopover.Control {
  var title: String {
    switch self {
    case .material: "Material"
    case .opacity: "Transparency"
    case .blur: "Blur"
    case .depth: "Depth"
    case .tint: "Tint"
    }
  }
}

#Preview("frosted") { PreviewMatrix { AppearancePopoverSample(material: .frosted) } }
#Preview("glossy") { PreviewMatrix { AppearancePopoverSample(material: .glossy) } }
#Preview("solid") { PreviewMatrix { AppearancePopoverSample(material: .solid) } }
#Preview("locked") {
  PreviewMatrix { AppearancePopover(state: PreviewState.appearanceLocked) { _ in } }
}
