// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The provider in the composer's model chip and the popover grouped by provider (T60.7, DS§3.7,
// DS§6.3, DS§6.4): the generated monogram's letters and colour rule, the chip ready and with the
// problem badge, and the popover with one provider greyed under "Add key".

import SwiftUI
import Testing

@testable import CoxUI

@Suite struct ProviderMonogramTests {
  @Test func initialsAreTheFirstLettersOfTheFirstTwoWords() {
    #expect(ProviderMonogram.letters(of: "Anthropic") == "A")
    #expect(ProviderMonogram.letters(of: "LM Studio") == "LS")
    #expect(ProviderMonogram.letters(of: "Jev System One") == "JS")
    #expect(ProviderMonogram.letters(of: "deepseek") == "D")
    #expect(ProviderMonogram.letters(of: "  ") == "?")
  }

  /// FNV-1a is the DS§3.7 rule, so a client on another platform draws the same colours: these
  /// are the values its reference implementation gives.
  @Test func theColourOfASlugIsFnv1aModuloTheTileKinds() {
    let kinds = IconTile.Kind.allCases
    #expect(ProviderMonogram.kind(of: "") == kinds[Int(2_166_136_261 % 5)])
    #expect(ProviderMonogram.kind(of: "anthropic") == ProviderMonogram.kind(of: "anthropic"))
    // `a` = 0x61: (2166136261 ^ 0x61) * 16777619 mod 2^32 = 0xE40C292C.
    #expect(ProviderMonogram.kind(of: "a") == kinds[Int(0xE40C_292C % 5)])
  }
}

@MainActor
@Suite struct ProviderChipSnapshotTests {
  @Test(arguments: Variant.all) func modelChipWithItsProvider(_ variant: Variant) throws {
    try check(
      ComposerChip("Sonnet 5 · high", kind: .model, provider: PreviewState.provider), variant,
      "ready")
  }

  @Test(arguments: Variant.all) func modelChipWithTheProblemBadge(_ variant: Variant) throws {
    try check(
      ComposerChip(
        "Sonnet 5 · high", kind: .model, provider: PreviewState.provider, hasProblem: true),
      variant, "badge")
  }

  @Test func monogramsOfSeveralProviders() throws {
    let marks = [
      PreviewState.provider, PreviewState.localProvider,
      ProviderMark(slug: "openai", name: "OpenAI"),
      ProviderMark(slug: "typesafe", name: "Jev System One"),
      ProviderMark(slug: "deepseek", name: "deepseek"),
    ]
    try check(
      HStack(spacing: Space.m) { ForEach(marks, id: \.slug) { ProviderMonogram($0) } },
      Variant(scheme: .light, material: .solid), "marks")
  }

  @Test func theChipRowShowsTheProviderAndTheBadge() throws {
    try assertCoxSnapshot(
      Composer(state: PreviewState.composerProviderProblem) { _ in }.padding(Space.xl),
      Variant(scheme: .light, material: .solid), named: "row-badge")
  }

  @Test(arguments: Variant.all) func popoverGroupsByProviderWithOneGreyedUnderAddKey(
    _ variant: Variant
  ) throws {
    try assertCoxSnapshot(
      ModelPopover(state: PreviewState.modelsOfTwoProviders) { _ in }, variant,
      named: variant.name)
  }

  @Test func onlyTheSectionWithoutAKeyOffersItAndNamesItsProvider() {
    let sections = PreviewState.modelsOfTwoProviders.sections
    #expect(sections.map(\.offersKey) == [false, true])
    #expect(sections.map(\.isEnabled) == [true, false])
    #expect(sections[1].provider == "openai")
  }

  private func check(
    _ chip: some View, _ variant: Variant, _ look: String, test: String = #function
  ) throws {
    try assertCoxSnapshot(
      PreviewPane { chip.fixedSize() }, variant, named: "\(look).\(variant.name)", testName: test)
  }
}
