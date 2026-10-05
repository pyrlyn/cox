// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Appearance popover with controls a higher config layer sets (T37.22.3, DS§3.5): those
// controls are disabled and the note names the layer, in both schemes.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct AppearancePopoverLockTests {
  @Test(arguments: [ColorScheme.light, .dark]) func aLockedControlIsDisabledAndNamesItsLayer(
    _ scheme: ColorScheme
  ) throws {
    try assertCoxSnapshot(
      AppearancePopover(state: PreviewState.appearanceLocked) { _ in },
      Variant(scheme: scheme, material: .frosted), named: scheme == .dark ? "dark" : "light")
  }
}
