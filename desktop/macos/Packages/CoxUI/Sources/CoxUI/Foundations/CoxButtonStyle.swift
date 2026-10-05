// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `CoxButtonStyle` (DS§6.1): every push button, the mockup's `.pb`, `.pb.pri` and `.pb.dan`,
// and the composer's round Send, `.send`. Separate so a button's face, label colour, lift and
// states come from one place, built on `elevation`, `specular`, `hairline` and `textStyle`.

import SwiftUI

/// The two button heights (DS§3.3): `size.buttonHeight` and `size.buttonHeightSmall`.
enum ButtonSize: CaseIterable, Sendable {
  case regular, small

  var height: CGFloat {
    switch self {
    case .regular: Size.buttonHeight
    case .small: Size.buttonHeightSmall
    }
  }
}

/// A push button of `role` and `size`, with hover, pressed and disabled states.
struct CoxButtonStyle: ButtonStyle {
  /// What the button does, which picks its face and label colour.
  enum Role: CaseIterable, Sendable {
    /// The default action: accent face.
    case primary
    /// Any other action: window-surface face with a hairline rim.
    case secondary
    /// A destructive action: the secondary face with a danger label.
    case danger
    /// A bare label that shows a face only on hover and press.
    case plain
  }

  let role: Role
  let size: ButtonSize

  init(_ role: Role = .secondary, size: ButtonSize = .regular) {
    self.role = role
    self.size = size
  }

  func makeBody(configuration: Configuration) -> some View {
    ControlStateReader(isPressed: configuration.isPressed) { state in
      ButtonFace(label: configuration.label, role: role, size: size, state: state)
    }
  }
}

/// The button drawn in one `ControlState`; the style picks the state, a snapshot sets it.
struct ButtonFace<Label: View>: View {
  let label: Label
  let role: CoxButtonStyle.Role
  let size: ButtonSize
  let state: ControlState
  @EffectiveAppearance private var appearance

  var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.m, style: .continuous)
    let lifted = role != .plain
    label
      .textStyle(.control)
      .foregroundStyle(role.foreground(state))
      .padding(.horizontal, Space.l)
      .frame(height: size.height)
      // Before the face and tint, so the sweep lies on them and under the label.
      .specular(lifted ? appearance.specular : MaterialToken.solidSpecular, in: shape)
      .background { shape.fill(state.tint) }
      .background {
        shape.fill(role.face(state).opacity(role.faceOpacity(appearance, state)))
      }
      .overlay { if role.hasRim(state) { Color.clear.hairline(in: shape) } }
      .elevation(state.elevation(lifted ? .e1 : .e0), cornerRadius: Radius.m)
      .contentShape(shape)
  }
}

extension CoxButtonStyle.Role {
  /// A disabled button keeps a readable label (DS§8) and drops only its colour.
  func foreground(_ state: ControlState) -> Color {
    switch (self, state) {
    case (_, .disabled): Color(.textSecondary)
    case (.primary, _): Color(.textOnAccent)
    case (.danger, _): Color(.statusDanger)
    case (.secondary, _), (.plain, _): Color(.textPrimary)
    }
  }

  /// A disabled primary loses its accent and shows the secondary face.
  private func faceRole(_ state: ControlState) -> Self {
    self == .primary && state == .disabled ? .secondary : self
  }

  func face(_ state: ControlState) -> Color {
    switch faceRole(state) {
    case .plain: .clear
    case .primary: Color(.accent)
    case .secondary, .danger: Color(.surfaceWindow)
    }
  }

  /// The face carries a label, so it holds the readable floor in every state (DS§3.5); the
  /// accent face is opaque.
  func faceOpacity(_ appearance: Appearance, _ state: ControlState) -> Double {
    switch faceRole(state) {
    case .plain: 0
    case .primary: MaterialToken.solidWindowOpacity
    case .secondary, .danger: appearance.backgroundOpacity(.readable)
    }
  }

  /// Window-surface faces get a hairline rim; the accent face and a bare label do not.
  func hasRim(_ state: ControlState) -> Bool {
    switch faceRole(state) {
    case .secondary, .danger: true
    case .primary, .plain: false
    }
  }
}

/// Send, the mockup's `.send`: a round accent face `Size.sendButton` wide; disabled,
/// `fill.secondary` with a readable glyph (DS§8).
struct SendButtonStyle: ButtonStyle {
  func makeBody(configuration: Configuration) -> some View {
    ControlStateReader(isPressed: configuration.isPressed) { state in
      configuration.label
        .foregroundStyle(state == .disabled ? Color(.textSecondary) : Color(.textOnAccent))
        .frame(width: Size.sendButton, height: Size.sendButton)
        .background { Circle().fill(state.tint) }
        .background {
          Circle().fill(state == .disabled ? Color(.fillSecondary) : Color(.accent))
        }
        .elevation(state.elevation(.e1), cornerRadius: Radius.capsule)
        .contentShape(Circle())
    }
  }
}
