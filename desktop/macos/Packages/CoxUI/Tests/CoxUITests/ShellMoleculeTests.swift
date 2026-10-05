// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The window shell's molecules' check (T37.21, DS§6.3): a snapshot per variant × light/dark ×
// Solid/Frosted, each molecule on a pane from `PreviewState` as its `#Preview` shows it, and
// the choices the molecules own.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct ShellMoleculeSnapshotTests {
  @Test(arguments: Variant.all) func sessionRow(_ variant: Variant) throws {
    try check(
      SessionRow(PreviewState.sessions[0], isSelected: true).frame(width: Size.sidebarWidth),
      variant, "selected")
    try check(SessionList(), variant, "list")
  }

  @Test(arguments: Variant.all) func sessionFilter(_ variant: Variant) throws {
    let prompt = PreviewState.filterPrompt
    try check(
      SessionFilter(text: .constant(""), prompt: prompt, shortcut: PreviewState.keys)
        .frame(width: Size.sidebarWidth), variant, "empty")
    try check(
      SessionFilter(text: .constant(PreviewState.filterText), prompt: prompt)
        .frame(width: Size.sidebarWidth), variant, "typed")
  }

  @Test(arguments: Variant.all) func breadcrumb(_ variant: Variant) throws {
    try check(BreadcrumbSample(branch: PreviewState.branch), variant, "branch")
    try check(BreadcrumbSample(branch: nil), variant, "project")
  }

  @Test(arguments: Variant.all) func modelCapsule(_ variant: Variant) throws {
    try check(ModelCapsule(PreviewState.model) {}, variant, "plain")
    try check(ModelCapsule(PreviewState.model, isOpen: true) {}, variant, "open")
  }

  @Test(arguments: Variant.all) func costCapsule(_ variant: Variant) throws {
    try check(CostCapsuleSample(isOpen: false), variant, "plain")
    try check(CostCapsuleSample(isOpen: true), variant, "open")
  }

  @Test(arguments: Variant.all) func modeSegmented(_ variant: Variant) throws {
    for mode in ModeSegmented.Mode.allCases {
      try check(ModeSegmented(selection: .constant(mode)), variant, "\(mode)")
    }
  }

  @Test(arguments: Variant.all) func stopButton(_ variant: Variant) throws {
    try check(StopButton {}, variant)
  }

  /// One image per molecule variant, named `<variant>.<cell>`, or `<cell>` for a one-look
  /// molecule. At its ideal size: the harness rounds a hosted view's size, which can leave a
  /// label half a point short, and a label a toolbar gives room to must not wrap here.
  private func check(
    _ molecule: some View, _ variant: Variant, _ look: String? = nil, test: String = #function
  ) throws {
    let name = [look, variant.name].compactMap(\.self).joined(separator: ".")
    try assertCoxSnapshot(
      PreviewPane { molecule.fixedSize() }, variant, named: name, testName: test)
  }
}

@Suite struct ShellMoleculeTests {
  @Test func bypassIsOfferedOnlyWhileItIsOn() {
    #expect(ModeSegmented.Mode.offered(.auto) == [.ask, .plan, .auto])
    #expect(ModeSegmented.Mode.offered(.bypass) == [.ask, .plan, .auto, .bypass])
  }

  @Test func eachModeIsMarkedInItsOwnColour() {
    let looks = ModeSegmented.Mode.allCases.map(\.look)
    #expect(
      looks == [
        .plain, .tinted(Color(.statusPlan)), .tinted(Color(.accent)), .filled(Color(.statusDanger)),
      ])
  }
}
