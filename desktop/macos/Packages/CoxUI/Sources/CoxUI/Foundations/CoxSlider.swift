// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `CoxSlider` (DS§6.1, the mockup's `.slider`): a sunken track, an `accent` fill up to the
// value and the 3D knob on top. A view rather than a style: macOS has no public `SliderStyle`,
// so this takes a slider's inputs and VoiceOver sees a slider. Disabled, it drops the fill and
// the knob's lift, as a disabled button drops its accent and lift (DS§6.1).

import SwiftUI

/// Drags `value` within `range`; `label` names it for VoiceOver.
struct CoxSlider: View {
  let label: LocalizedStringKey
  @Binding var value: Double
  let range: ClosedRange<Double>
  @Environment(\.isEnabled) private var isEnabled

  /// The mockup's 22 pt row, 6 pt track and 20 pt knob.
  private static let height: CGFloat = 22
  private static let track: CGFloat = 6
  private static let knob: CGFloat = 20
  /// How far the fill's leading end is lightened towards white (the mockup's `#6fb4ff`).
  private static let fillLift = 0.4

  init(_ label: LocalizedStringKey, value: Binding<Double>, in range: ClosedRange<Double> = 0...1) {
    self.label = label
    self._value = value
    self.range = range
  }

  var body: some View {
    GeometryReader { geometry in
      let travel = max(0, geometry.size.width - Self.knob)
      let centre = Self.knob / 2 + travel * fraction
      ZStack(alignment: .leading) {
        Color.clear
          .frame(height: Self.track)
          .insetWell(Color(.fillSecondary), cornerRadius: Self.track / 2)
        if isEnabled {
          Capsule()
            .fill(
              LinearGradient(
                colors: [Color(.accent).mix(with: .white, by: Self.fillLift), Color(.accent)],
                startPoint: .leading, endPoint: .trailing)
            )
            .frame(width: centre, height: Self.track)
        }
        Knob(diameter: Self.knob).offset(x: centre - Self.knob / 2)
      }
      .frame(maxHeight: .infinity)
      .contentShape(Rectangle())
      .gesture(
        DragGesture(minimumDistance: 0).onChanged { drag in
          value = value(atFraction: (drag.location.x - Self.knob / 2) / max(travel, 1))
        })
    }
    .frame(height: Self.height)
    .accessibilityRepresentation {
      Slider(value: $value, in: range) { Text(label) }
    }
  }

  /// Where `value` sits in `range`, 0…1.
  private var fraction: Double {
    let span = range.upperBound - range.lowerBound
    guard span > 0 else { return 0 }
    return min(max((value - range.lowerBound) / span, 0), 1)
  }

  /// The value at `fraction` of the track, clamped to `range`.
  private func value(atFraction fraction: Double) -> Double {
    range.lowerBound + (range.upperBound - range.lowerBound) * min(max(fraction, 0), 1)
  }
}
