// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `LabeledSlider` (DS§6.3 row `LabeledSlider`, the mockup's `.appear .lbl`, `.slider` and
// `.ends`): a setting on a scale — its name and current value above the slider, what each end
// means below it. Separate so every Appearance scale (transparency, blur, reflection, Depth)
// reads the same way.

import SwiftUI

/// A `SectionHeader` with the value at its trailing edge, a `CoxSlider`, then the two ends in
/// `font.micro`.
struct LabeledSlider: View {
  let title: String
  @Binding var value: Double
  let range: ClosedRange<Double>
  /// The value as the core formats it, `58%`, `34 pt`, `Strong`.
  let valueText: String
  /// What the low and high ends mean, `Opaque` and `Clear`, or `nil`.
  let ends: (low: String, high: String)?

  init(
    _ title: String, value: Binding<Double>, in range: ClosedRange<Double> = 0...1,
    valueText: String, ends: (low: String, high: String)? = nil
  ) {
    self.title = title
    self._value = value
    self.range = range
    self.valueText = valueText
    self.ends = ends
  }

  var body: some View {
    VStack(spacing: Space.xs) {
      SectionHeader(title) {
        Text(valueText)
          .textStyle(.label, tabularDigits: true)
          .foregroundStyle(Color(.textPrimary))
          .accessibilityHidden(true)
      }
      CoxSlider(LocalizedStringKey(title), value: $value, in: range)
        .accessibilityValue(valueText)
      if let ends {
        HStack {
          Text(ends.low)
          Spacer(minLength: Space.m)
          Text(ends.high)
        }
        .textStyle(.micro)
        .foregroundStyle(Color(.textTertiary))
        .accessibilityHidden(true)
      }
    }
  }
}

#Preview("ends") { PreviewMatrix { LabeledSliderSample(ends: PreviewState.sliderEnds) } }
#Preview("no ends") { PreviewMatrix { LabeledSliderSample(ends: nil) } }
