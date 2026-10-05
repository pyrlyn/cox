// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The inspector rows' check (T37.21.9, DS§6.3): a snapshot per variant × light/dark ×
// Solid/Frosted of `ChangedFileRow` and `CheckpointRow`, on a pane from `PreviewState` as their
// `#Preview`s show them, and the path split a changed file owns.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct InspectorRowSnapshotTests {
  @Test(arguments: Variant.all) func changedFileRow(_ variant: Variant) throws {
    try check(ChangedFileSample(isSelected: true), variant, "selected")
    try check(ChangedFileList(), variant, "list")
    try check(DeletedFileSample(), variant, "deleted")
  }

  @Test(arguments: Variant.all) func checkpointRow(_ variant: Variant) throws {
    try check(CheckpointSample(isSelected: true), variant, "selected")
    try check(CheckpointList(), variant, "list")
  }

  /// One image per look, named `<look>.<cell>`, at its ideal size (see
  /// `ShellMoleculeSnapshotTests.check`).
  private func check(
    _ molecule: some View, _ variant: Variant, _ look: String, test: String = #function
  ) throws {
    try assertCoxSnapshot(
      PreviewPane { molecule.fixedSize() }, variant, named: "\(look).\(variant.name)",
      testName: test)
  }
}

@Suite struct InspectorRowTests {
  @Test func aPathDimsItsDirectoryAndKeepsTheFileName() {
    let file = ChangedFileRow.File(
      path: "cox-provider-http/src/retry.rs", change: .edited, added: 1, removed: 0)
    let runs = file.styledPath.runs.map { String(file.styledPath[$0.range].characters) }
    #expect(runs == ["cox-provider-http/src/", "retry.rs"])
    #expect(file.styledPath.runs.first?.foregroundColor == Color(.textSecondary))
    #expect(file.styledPath.runs.last?.foregroundColor == nil)
  }

  @Test func aPathWithoutADirectoryIsAllFileName() {
    let file = ChangedFileRow.File(path: "Cargo.toml", change: .created, added: 1, removed: 0)
    #expect(String(file.styledPath.characters) == "Cargo.toml")
    #expect(file.styledPath.runs.count == 1)
    #expect(file.styledPath.runs.first?.foregroundColor == nil)
  }
}
