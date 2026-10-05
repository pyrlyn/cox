// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `[desktop.appearance]` (DS§3.5, T37.13) as the Appearance popover edits it: one edit per key,
// written through `SettingsStore.set` like any other setting, and the section read back from
// the settings view so the window redraws from what Rust stored (T37.26). Separate so the
// popover's intents reach the config without a view naming a key; the app maps these plain
// values to CoxUI's, which CoxModel does not import.

import CoxClient

/// The window material, spelled as `desktop.appearance.material` stores it.
public enum WindowMaterial: String, CaseIterable, Sendable {
  case frosted, glossy, solid
}

/// One change the Appearance popover reports, as the key it writes.
public enum AppearanceEdit: Equatable, Sendable {
  case material(WindowMaterial)
  /// Window and pane background opacity, 0 (clear) … 1 (opaque).
  case opacity(Double)
  /// Blur in pt for Frosted; Glossy reads it as reflection.
  case blur(Double)
  /// 0 (Flat) … 1 (3D).
  case depth(Double)
  case tint(Bool)

  public var key: String {
    let field =
      switch self {
      case .material: "material"
      case .opacity: "opacity"
      case .blur: "blur"
      case .depth: "depth"
      case .tint: "tint"
      }
    return DesktopAppearance.key(field)
  }

  var value: SettingValue {
    switch self {
    case .material(let material): .text(material.rawValue)
    case .opacity(let number), .blur(let number), .depth(let number): .number(number)
    case .tint(let isOn): .bool(isOn)
    }
  }
}

/// `[desktop.appearance]` as Rust stored it.
public struct DesktopAppearance: Equatable, Sendable {
  public var material: WindowMaterial
  public var opacity: Double
  public var blur: Double
  /// The blur the schema allows: the slider's range.
  public var blurRange: ClosedRange<Double>
  public var depth: Double
  public var tint: Bool

  static func key(_ field: String) -> String { "desktop.appearance.\(field)" }
}

extension DesktopAppearance {
  /// `nil` when a key is missing or holds a value of another type.
  init?(_ settings: [Setting]) {
    let rows = SectionRows(settings, "desktop.appearance")
    guard let name: String = rows.decode("material"),
      let material = WindowMaterial(rawValue: name),
      let opacity: Double = rows.decode("opacity"), let blur: Double = rows.decode("blur"),
      let depth: Double = rows.decode("depth"), let tint: Bool = rows.decode("tint"),
      case .number(let low?, let high?) = rows["blur"]?.kind, low <= high
    else { return nil }
    (self.material, self.opacity, self.blur, self.depth, self.tint) =
      (material, opacity, blur, depth, tint)
    blurRange = low...high
  }
}

/// `[desktop.appearance] dark_highlight`, spelled as Rust stores it (A109).
public enum DarkHighlightSetting: String, Equatable, Sendable, Decodable {
  /// No highlight in dark mode: the dark mockup's look.
  case none
  /// White at a tenth of the light highlight's strength.
  case subtle
}

/// `[desktop.appearance] dark_highlight_scope`, spelled as Rust stores it (A109).
public enum DarkHighlightScope: String, Equatable, Sendable, Decodable {
  /// Controls only (e1).
  case controls
  /// Every lifted level (e1–e4), the transcript's user bubble included.
  case all
}

extension SettingsStore {
  /// `[desktop.appearance] dark_highlight`; none before the first load or when the key holds
  /// something else.
  public var darkHighlight: DarkHighlightSetting {
    view.flatMap { SectionRows($0.settings, "desktop.appearance").decode("dark_highlight") }
      ?? .none
  }

  /// `[desktop.appearance] dark_highlight_scope`; controls before the first load or when the key
  /// holds something else.
  public var darkHighlightScope: DarkHighlightScope {
    view.flatMap {
      SectionRows($0.settings, "desktop.appearance").decode("dark_highlight_scope")
    } ?? .controls
  }

  /// `[desktop.appearance]` from the loaded view; `nil` before the first load.
  public var appearance: DesktopAppearance? { view.flatMap { DesktopAppearance($0.settings) } }

  /// Writes one popover change to the user's config; `appearance` follows Rust's answer.
  public func apply(_ edit: AppearanceEdit) async {
    await set(edit.key, to: edit.value)
  }
}
