// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Info tab's check (T37.29.5, DT§5.1, DS§6.4): the inspector on its Info tab per
// light/dark × Solid/Frosted cell, and empty.

import Testing

@testable import CoxUI

@MainActor
@Suite struct InfoTabSnapshotTests {
  @Test(arguments: Variant.all) func infoTab(_ variant: Variant) throws {
    try assertCoxSnapshot(
      InfoInspectorSample(state: PreviewState.info), variant, named: variant.name)
  }

  @Test func emptyInfoTab() throws {
    try assertCoxSnapshot(
      InfoInspectorSample(state: .init()), Variant.all[0], named: Variant.all[0].name)
  }
}
