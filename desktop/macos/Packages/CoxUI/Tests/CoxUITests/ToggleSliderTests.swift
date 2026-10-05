// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `CoxToggleStyle` and `CoxSlider`'s check (T37.19.4): a snapshot per on/off and per slider
// value × light/dark × Solid/Frosted, disabled ones too (T37.19.5), and the slider placing its
// knob by the value's share of the range, clamped to it.

import AppKit
import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct ToggleSliderTests {
  @Test(arguments: Variant.all) func toggle(_ variant: Variant) throws {
    for isOn in [false, true] {
      try assertCoxSnapshot(
        ToggleSample(isOn: isOn), variant, named: "\(isOn ? "on" : "off").\(variant.name)")
    }
  }

  @Test(arguments: Variant.all) func slider(_ variant: Variant) throws {
    for value in [0, 0.5, 1] {
      try assertCoxSnapshot(
        SliderSample(value: value), variant, named: "value\(Int(value * 100)).\(variant.name)")
    }
  }

  @Test(arguments: Variant.all) func disabledToggle(_ variant: Variant) throws {
    for isOn in [false, true] {
      try assertCoxSnapshot(
        ToggleSample(isOn: isOn).disabled(true), variant,
        named: "\(isOn ? "on" : "off").\(variant.name)")
    }
  }

  @Test(arguments: Variant.all) func disabledSlider(_ variant: Variant) throws {
    try assertCoxSnapshot(
      SliderSample(value: 0.5).disabled(true), variant, named: "value50.\(variant.name)")
  }

  @Test func sliderPlacesTheKnobByTheValuesShareOfTheRange() throws {
    try #expect(
      render(SliderSample(value: 15, range: 10...20)) == render(SliderSample(value: 0.5)))
  }

  @Test func sliderClampsAValueOutsideTheRange() throws {
    try #expect(render(SliderSample(value: 1.5)) == render(SliderSample(value: 1)))
    try #expect(render(SliderSample(value: -1)) == render(SliderSample(value: 0)))
  }

  private func render(_ sample: SliderSample) throws -> Data? {
    try SnapshotHost(sample, Variant(scheme: .light, material: .solid)).bitmap()
      .tiffRepresentation
  }
}

private struct ToggleSample: View {
  let isOn: Bool

  var body: some View {
    Toggle("Tint from wallpaper", isOn: .constant(isOn)).toggleStyle(CoxToggleStyle())
  }
}

private struct SliderSample: View {
  let value: Double
  var range: ClosedRange<Double> = 0...1

  var body: some View {
    CoxSlider("Depth", value: .constant(value), in: range).frame(width: Size.popoverWidth / 2)
  }
}
