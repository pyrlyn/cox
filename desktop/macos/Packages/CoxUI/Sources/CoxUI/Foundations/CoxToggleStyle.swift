// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `CoxToggleStyle` (DS§6.1, the mockup's `.tog`): the switch, a sunken track that fills with
// `accent` when on and the 3D knob that moves across it — sliding, or cross-fading under Reduce
// Motion. Separate so every on/off setting looks and moves alike. Disabled, it keeps its knob's
// place but loses the accent and the lift, as a disabled button does (DS§6.1).

import SwiftUI

/// The label, then the switch.
struct CoxToggleStyle: ToggleStyle {
  func makeBody(configuration: Configuration) -> some View {
    HStack(spacing: Space.m) {
      ToggleLabel(label: configuration.label)
      Switch(isOn: configuration.$isOn)
    }
    .accessibilityRepresentation {
      Toggle(isOn: configuration.$isOn) { configuration.label }.toggleStyle(.switch)
    }
  }
}

/// The setting's name; a disabled one dims its colour only (DS§8).
private struct ToggleLabel<Label: View>: View {
  let label: Label
  @Environment(\.isEnabled) private var isEnabled

  var body: some View {
    label
      .textStyle(.body)
      .foregroundStyle(Color(isEnabled ? .textPrimary : .textSecondary))
  }
}

private struct Switch: View {
  @Binding var isOn: Bool
  @Namespace private var knob
  @Environment(\.isEnabled) private var isEnabled

  /// The mockup's 32 × 19 track with a 2 pt gap around the knob.
  private static let width: CGFloat = 32
  private static let height: CGFloat = 19
  private static let gap: CGFloat = 2

  var body: some View {
    HStack(spacing: 0) {
      if isOn {
        Spacer(minLength: 0)
        thumb
      } else {
        thumb
        Spacer(minLength: 0)
      }
    }
    .padding(Self.gap)
    .frame(width: Self.width, height: Self.height)
    .insetWell(Color(isOn && isEnabled ? .accent : .fillSecondary), cornerRadius: Self.height / 2)
    .contentShape(Capsule())
    .onTapGesture { isOn.toggle() }
    .animation(.cox(Motion.durationFast), value: isOn)
  }

  private var thumb: some View {
    Knob(diameter: Self.height - Self.gap * 2).coxMatchedGeometry(id: 0, in: knob)
  }
}
