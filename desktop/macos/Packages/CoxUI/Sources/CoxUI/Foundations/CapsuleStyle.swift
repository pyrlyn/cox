// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `CapsuleStyle` (DS§6.1): the toolbar capsules and filter chips, the mockup's `.cap` and
// `.cap.hot`. Separate so every capsule has one glass face, lift and active halo, built on
// `glassPane`, `hairline`, `elevation` and `textStyle`, with the states of `ControlState`.

import SwiftUI

/// A capsule button, plain or active (its popover is open, its filter is on).
struct CapsuleStyle: ButtonStyle {
  enum Emphasis: CaseIterable, Sendable {
    /// At rest in the toolbar: a glass capsule.
    case plain
    /// Open or on: a brighter face, accent label and an accent halo.
    case active
  }

  let emphasis: Emphasis
  /// The mockup's `.cap.icon`: a symbol alone in a circle as wide as the capsule is tall.
  let isIcon: Bool

  /// The mockup's `.cap.hot` ring: `0 0 0 3px` of the focus-halo colour.
  fileprivate static let haloWidth: CGFloat = 3

  init(_ emphasis: Emphasis = .plain, isIcon: Bool = false) {
    self.emphasis = emphasis
    self.isIcon = isIcon
  }

  func makeBody(configuration: Configuration) -> some View {
    ControlStateReader(isPressed: configuration.isPressed) { state in
      CapsuleFace(
        label: configuration.label, emphasis: emphasis, state: state, isIcon: isIcon)
    }
  }
}

/// The capsule drawn in one `ControlState`; the style picks the state, a snapshot sets it.
struct CapsuleFace<Label: View>: View {
  let label: Label
  let emphasis: CapsuleStyle.Emphasis
  let state: ControlState
  var isIcon = false

  var body: some View {
    // The same continuous shape `elevation` draws its highlight in, so the layers line up.
    let shape = RoundedRectangle(cornerRadius: Radius.capsule, style: .continuous)
    label
      .textStyle(.control)
      .foregroundStyle(emphasis.foreground(state))
      .padding(.horizontal, isIcon ? 0 : Space.l)
      .frame(width: isIcon ? Size.capsuleHeight : nil, height: Size.capsuleHeight)
      .background { shape.fill(state.tint) }
      .glassPane(shape, surface: emphasis.surface, role: .readable)
      .hairline(in: shape, color: Color(.surfaceCapsuleBorder))
      .elevation(state.elevation(.e1), cornerRadius: Radius.capsule)
      .background {
        // Filled behind the face, not stroked: a stroked pill renders with stray side bars.
        if emphasis == .active {
          shape.fill(Color(.accentSoft)).padding(-CapsuleStyle.haloWidth)
        }
      }
      .contentShape(shape)
  }
}

extension CapsuleStyle.Emphasis {
  /// The capsule carries a label, so a disabled one dims its colour only (DS§8).
  func foreground(_ state: ControlState) -> Color {
    switch (self, state) {
    case (_, .disabled): Color(.textSecondary)
    case (.active, _): Color(.accent)
    case (.plain, _): Color(.textPrimary)
    }
  }

  /// Active lifts to the window surface, brighter than the capsule glass.
  var surface: Color {
    switch self {
    case .plain: Color(.surfaceCapsule)
    case .active: Color(.surfaceWindow)
    }
  }
}
