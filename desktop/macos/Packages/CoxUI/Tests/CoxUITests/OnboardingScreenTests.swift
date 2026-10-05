// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The first-run window's check (T37.31, DT§5.8): with no provider key the checklist says what is
// missing and offers Settings; with every check passing it offers nothing but the project step.
// Both in every light/dark × Solid/Frosted cell, and the row in each status. The project drop zone
// (T37.45.5) idle and hovered, and what a drop opens: one directory, as the picker would.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct OnboardingScreenSnapshotTests {
  @Test(arguments: Variant.all) func noProvider(_ variant: Variant) throws {
    try check(PreviewState.onboardingNoProvider, variant)
  }

  @Test(arguments: Variant.all) func allGreen(_ variant: Variant) throws {
    try check(PreviewState.onboardingAllGreen, variant)
  }

  @Test(arguments: Variant.all) func checklistRows(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane {
        SettingsGroupBox(PreviewState.checksTitle) {
          ForEach(ChecklistRow.Status.allCases, id: \.self) { ChecklistRowSample(status: $0) }
        }
        .fixedSize()
      }, variant, named: variant.name)
  }

  @Test(arguments: Variant.all) func dropZoneIdle(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane { ProjectDropZone { _ in }.frame(width: Size.readingWidth) }, variant,
      named: variant.name)
  }

  @Test(arguments: Variant.all) func dropZoneHovered(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane { ProjectDropZone(isTargeted: true) { _ in }.frame(width: Size.readingWidth) },
      variant, named: variant.name)
  }

  private func check(
    _ state: OnboardingScreenState, _ variant: Variant, test: String = #function
  ) throws {
    try assertCoxWindowSnapshot(
      OnboardingScreen(state: state) { _ in }, variant,
      size: CGSize(width: Size.windowSmallWidth, height: Size.windowMinHeight), testName: test)
  }
}

@Suite struct OnboardingScreenTests {
  @Test func eachFixReportsItsIntent() {
    #expect(OnboardingScreen.Fix.openSettings.intent == .openSettings)
    #expect(OnboardingScreen.Fix.retry.intent == .retry)
  }

  @Test func aDroppedDirectoryOpensAsThePickedFolderDoes() throws {
    let folder = try Scratch.make(directory: true)
    defer { Scratch.remove(folder) }
    // `.openFolder` is the intent the app sends for the folder its picker chose.
    #expect(ProjectDropZone.intent(for: [folder]) == .openFolder(folder))
  }

  @Test func aDroppedFileIsRefused() throws {
    let file = try Scratch.make(directory: false)
    defer { Scratch.remove(file) }
    #expect(ProjectDropZone.intent(for: [file]) == nil)
  }

  @Test func aWebLinkOrSeveralFoldersAreRefused() throws {
    let one = try Scratch.make(directory: true)
    let two = try Scratch.make(directory: true)
    defer {
      Scratch.remove(one)
      Scratch.remove(two)
    }
    #expect(ProjectDropZone.intent(for: [one, two]) == nil)
    #expect(ProjectDropZone.intent(for: []) == nil)
    let web = try #require(URL(string: "https://example.com/repo"))
    #expect(ProjectDropZone.intent(for: [web]) == nil)
  }
}

/// A uniquely named directory or empty file under the temporary directory, for a drop to name.
private enum Scratch {
  static func make(directory: Bool) throws -> URL {
    let url = FileManager.default.temporaryDirectory.appending(
      path: "cox-drop-\(UUID().uuidString)", directoryHint: directory ? .isDirectory : .notDirectory
    )
    if directory {
      try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    } else {
      try Data().write(to: url)
    }
    return url
  }

  static func remove(_ url: URL) { try? FileManager.default.removeItem(at: url) }
}
