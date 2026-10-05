// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `CoxButtonStyle`'s check (T37.19.1): one snapshot per role × size × light/dark × Solid/Frosted,
// each showing rest, hovered, pressed and disabled; the disabled face holds the readable floor.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct ButtonStyleSnapshotTests {
  @Test(arguments: ButtonSize.allCases, Variant.all)
  func primary(_ size: ButtonSize, _ variant: Variant) throws {
    try check(.primary, size, variant)
  }

  @Test(arguments: ButtonSize.allCases, Variant.all)
  func secondary(_ size: ButtonSize, _ variant: Variant) throws {
    try check(.secondary, size, variant)
  }

  @Test(arguments: ButtonSize.allCases, Variant.all)
  func danger(_ size: ButtonSize, _ variant: Variant) throws {
    try check(.danger, size, variant)
  }

  @Test(arguments: ButtonSize.allCases, Variant.all)
  func plain(_ size: ButtonSize, _ variant: Variant) throws {
    try check(.plain, size, variant)
  }

  private func check(
    _ role: CoxButtonStyle.Role, _ size: ButtonSize, _ variant: Variant,
    testName: String = #function
  ) throws {
    try assertCoxSnapshot(
      ButtonSample(role: role, size: size), variant, named: "\(size)-\(variant.name)",
      testName: testName)
  }
}

@Suite struct ButtonStyleTests {
  @Test func disabledFaceHoldsTheReadableFloor() {
    let clear = Appearance(material: .frosted, windowOpacity: 0)
    for role in CoxButtonStyle.Role.allCases where role != .plain {
      #expect(
        role.faceOpacity(clear, .disabled) >= MaterialToken.readableFloorWindowOpacity)
      #expect(role.foreground(.disabled) == Color(.textSecondary))
    }
  }

  @Test func disabledWinsOverPressedAndPressedOverHovered() {
    #expect(ControlState(isEnabled: false, isPressed: true, isHovered: true) == .disabled)
    #expect(ControlState(isEnabled: true, isPressed: true, isHovered: true) == .pressed)
    #expect(ControlState(isEnabled: true, isPressed: false, isHovered: true) == .hovered)
    #expect(ControlState(isEnabled: true, isPressed: false, isHovered: false) == .rest)
  }

  @Test func pressedAndDisabledFacesSitOnTheSurface() {
    #expect(ControlState.pressed.elevation(.e1) == .e0)
    #expect(ControlState.disabled.elevation(.e1) == .e0)
    #expect(ControlState.hovered.elevation(.e1) == .e1)
  }
}

/// One role at one size in every state, on a pane as buttons sit in the app.
private struct ButtonSample: View {
  let role: CoxButtonStyle.Role
  let size: ButtonSize

  var body: some View {
    PreviewPane {
      HStack(spacing: Space.m) {
        ForEach(ControlState.allCases, id: \.self) { state in
          ButtonFace(label: Text("Allow"), role: role, size: size, state: state)
        }
      }
    }
  }
}
