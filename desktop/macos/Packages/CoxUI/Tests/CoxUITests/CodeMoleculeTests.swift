// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The code molecules' check (T37.21.2, T37.21.3, DS§6.3): a snapshot per variant × light/dark ×
// Solid/Frosted, each molecule on a pane from `PreviewState` as its `#Preview` shows it, and how
// highlighted runs become text.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct CodeMoleculeSnapshotTests {
  @Test(arguments: Variant.all) func diffLine(_ variant: Variant) throws {
    for kind in DiffLineView.Kind.allCases {
      try check(DiffLineSample(kind: kind), variant, "\(kind)")
    }
  }

  @Test(arguments: Variant.all) func diffHunk(_ variant: Variant) throws {
    try check(DiffHunkSample(), variant)
  }

  /// Review's hunk header (T51.21): "Revert hunk" laid out but hidden at rest, shown on hover.
  @Test(arguments: Variant.all) func diffHunkHeaderWithRevert(_ variant: Variant) throws {
    try check(DiffHunkSample(revert: true), variant, "idle")
    try check(DiffHunkSample(revert: true, isHovered: true), variant, "hovered")
  }

  @Test(arguments: Variant.all) func codeBlock(_ variant: Variant) throws {
    try check(CodeBlockSample(language: PreviewState.codeLanguage), variant, "language")
    try check(CodeBlockSample(language: nil), variant, "plain")
  }

  /// One image per molecule variant, named `<variant>.<cell>`, or `<cell>` for a one-look
  /// molecule, at its ideal size (see `ShellMoleculeSnapshotTests`).
  private func check(
    _ molecule: some View, _ variant: Variant, _ look: String? = nil, test: String = #function
  ) throws {
    let name = [look, variant.name].compactMap(\.self).joined(separator: ".")
    try assertCoxSnapshot(
      PreviewPane { molecule.fixedSize() }, variant, named: name, testName: test)
  }
}

@Suite struct CodeRunTests {
  @Test func linesJoinWithNewlinesAndOnlyHighlightedRolesCarryAColour() {
    let text = CodeRun.attributed([
      [CodeRun("let", .keyword), CodeRun(" x")], [CodeRun("}")],
    ])
    #expect(String(text.characters) == "let x\n}")
    let colours = text.runs.map(\.foregroundColor)
    #expect(colours == [Color(.syntaxKeyword), nil])
  }

  @Test func onlyAChangedRunSitsOnTheMark() {
    let text = CodeRun.attributed(
      [[CodeRun("let "), CodeRun("new", isChanged: true), CodeRun(" = 1")]], mark: .red)
    #expect(text.runs.map(\.backgroundColor) == [nil, .red, nil])
  }
}
