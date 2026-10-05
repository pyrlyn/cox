// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `Knob` (DS§6.1, the mockup's `.tog:after` and `.slider .kn`): the 3D knob that toggles and
// sliders share, so both lift, shade and respond to Depth the same way.

import SwiftUI

/// A white disc shaded top to bottom, rimmed with a hairline and lifted to e1; flat when
/// disabled, as `ControlState` lowers any disabled face.
struct Knob: View {
  let diameter: CGFloat
  @Environment(\.isEnabled) private var isEnabled

  /// The mockup's `#fff → #eceff6`: white darkened a little towards the shadow tint.
  private static let shade = 0.07

  var body: some View {
    Circle()
      .fill(
        LinearGradient(
          colors: [.white, Color.white.mix(with: Color(.shadowTint), by: Self.shade)],
          startPoint: .top, endPoint: .bottom)
      )
      .hairline(in: Circle())
      .frame(width: diameter, height: diameter)
      .elevation(
        ControlState(isEnabled: isEnabled, isPressed: false, isHovered: false).elevation(.e1),
        cornerRadius: diameter / 2)
  }
}
