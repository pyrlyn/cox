// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The model popover's check (T37.22.6, DT§5.1): its tiers in every light/dark × Solid/Frosted
// cell, the empty state, and the main screen with it hung under the toolbar's model capsule.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct ModelPopoverSnapshotTests {
  @Test(arguments: Variant.all) func modelPopover(_ variant: Variant) throws {
    try assertCoxSnapshot(
      ModelPopover(state: PreviewState.models) { _ in }, variant, named: variant.name)
  }

  @Test func emptyCatalogSaysSo() throws {
    try assertCoxSnapshot(
      ModelPopover(state: ModelPopover.State()) { _ in },
      Variant(scheme: .light, material: .solid), named: "light")
  }

  @Test func mainScreenHangsItUnderTheCapsule() throws {
    try assertCoxWindowSnapshot(
      MainScreen(
        state: PreviewState.modelOpen, send: { _ in }, transcript: { EmptyView() },
        inspector: { _ in }),
      Variant(scheme: .light, material: .solid))
  }
}
