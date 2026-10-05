// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `[desktop.appearance]` between `SettingsStore` and the Appearance popover (T37.26, DS§3.5):
// the stored values and their texts into CoxUI's state, a control a higher layer sets marked
// locked with that layer, a popover change back to the key it writes, and the write coalescing a
// slider drag needs. Here because CoxModel does not import CoxUI and CoxUI no cox package; the
// app is where the two meet.

import CoxClient
import CoxModel
import CoxUI
import Foundation

extension AppearancePopover.State {
  /// The popover over the loaded config; the defaults before the first load.
  @MainActor init(_ settings: SettingsStore) {
    self.init()
    guard let stored = settings.appearance else { return }
    material = GlassMaterial(rawValue: stored.material.rawValue) ?? .frosted
    (opacity, blur, blurRange) = (stored.opacity, stored.blur, stored.blurRange)
    (depth, tint) = (stored.depth, stored.tint)
    fillTexts()
    let appearance = settings.view?.settings.filter { $0.key.hasPrefix("desktop.appearance.") }
    for setting in appearance ?? [] where !setting.editable {
      let field = setting.key.split(separator: ".").last.map(String.init) ?? ""
      guard let control = AppearancePopover.Control(rawValue: field) else { continue }
      locked[control] = Self.name(setting.layer)
    }
  }

  /// The value texts beside the sliders, from the values themselves, so they follow a drag.
  mutating func fillTexts() {
    transparencyText = Self.percent(1 - opacity)
    blurText =
      material == .glossy
      ? Self.percent(Self.fraction(blur, in: blurRange)) : "\(Int(blur.rounded())) pt"
    depthText = Self.percent(depth)
  }

  /// How much of the system's behind-window blur the window draws, 0…1. AppKit blurs at one
  /// radius, about Frosted's default, so that default and anything heavier take all of it —
  /// nothing behind the window reads sharp — and a lighter blur fades it out. Glossy's value is
  /// its reflection, so its window takes the material's own light blur (DS§3.5).
  var blurFraction: Double {
    switch material {
    case .frosted: min(max(blur / MaterialToken.frostedBlur, 0), 1)
    case .glossy: MaterialToken.glossyBlur / MaterialToken.frostedBlur
    case .solid: 0
    }
  }

  static func fraction(_ value: Double, in range: ClosedRange<Double>) -> Double {
    guard range.upperBound > range.lowerBound else { return 0 }
    return min(max((value - range.lowerBound) / (range.upperBound - range.lowerBound), 0), 1)
  }

  private static func percent(_ value: Double) -> String {
    value.formatted(.percent.precision(.fractionLength(0)))
  }

  /// The layer as the popover's note names it.
  private static func name(_ layer: Layer) -> String {
    switch layer {
    case .default: "default"
    case .user: "user"
    case .project: "project"
    case .env: "environment"
    case .flag: "launch flag"
    case .claudeSettings: "Claude settings"
    }
  }
}

extension AppearanceEdit {
  init(_ change: AppearancePopover.Intent) {
    switch change {
    case .material(let material):
      self = .material(WindowMaterial(rawValue: material.rawValue) ?? .frosted)
    case .opacity(let value): self = .opacity(value)
    case .blur(let value): self = .blur(value)
    case .depth(let value): self = .depth(value)
    case .tint(let isOn): self = .tint(isOn)
    }
  }
}

/// Holds each key's latest value until the control rests, then writes it once: a slider reports
/// every drag step, and each write is a config file edit in Rust (T37.30.1).
@MainActor
final class Coalescer {
  /// Long enough to span a drag's steps, short enough that a release saves at once.
  private static let rest = Duration.milliseconds(150)
  private var pending: [String: (serial: Int, task: Task<Void, Never>)] = [:]
  private var serial = 0

  /// Whether a write waits or runs: the view keeps its own value until Rust answers.
  var isPending: Bool { !pending.isEmpty }
  /// Runs once the last write has returned, so the view reads back what Rust stored, or the
  /// old value when Rust refused the new one.
  var onIdle: (@MainActor () -> Void)?

  func submit(_ key: String, _ write: @escaping @MainActor () async -> Void) {
    pending[key]?.task.cancel()
    serial += 1
    let mine = serial
    let task = Task { [weak self] in
      try? await Task.sleep(for: Self.rest)
      guard !Task.isCancelled else { return }
      await write()
      // A newer value for the key may have replaced this one while it wrote.
      guard let self, pending[key]?.serial == mine else { return }
      pending[key] = nil
      if pending.isEmpty { onIdle?() }
    }
    pending[key] = (mine, task)
  }
}
