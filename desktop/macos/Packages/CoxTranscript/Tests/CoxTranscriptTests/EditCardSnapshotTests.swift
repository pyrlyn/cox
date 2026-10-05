// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// An opened edit card (T37.23.5, DS§6.4 row `ToolCard`): the `edit` block of the recorded
// `edit.json` fixture, whose `DiffModel` `TranscriptCard` maps to `DiffHunkView`s, in light and
// dark, Solid (DS§9). Separate from the transcript snapshot because a card in the text starts
// folded and opens only by its own state.

import AppKit
import CoxClient
import CoxUI
import SnapshotTesting
import SwiftUI
import Testing

@testable import CoxTranscript

/// `desktop/macos/Fixtures/edit.json`, found from this file so the test reads the recorder's
/// output in place.
private let editFixture = URL(filePath: #filePath)
  .deletingLastPathComponent()  // CoxTranscriptTests
  .deletingLastPathComponent()  // Tests
  .deletingLastPathComponent()  // CoxTranscript
  .deletingLastPathComponent()  // Packages
  .deletingLastPathComponent()  // macos
  .appending(path: "Fixtures/edit.json")

/// The fixture's edit: the core split, numbered and highlighted its one hunk.
private func editCard() throws -> ToolCard.Content {
  let blocks = try Fixture(contentsOf: editFixture).snapshot
  let edit = try #require(
    blocks.first {
      guard case .tool(let tool, _, _, _, _, _, _, let diff, _, _) = $0.kind else { return false }
      return tool == "edit" && diff != nil
    })
  return try #require(ToolCard.Content(edit, locale: Locale(identifier: "en_US_POSIX")))
}

// The reading column's width; the height follows the card.
// swiftlint:disable:next no_literal_size
private let size = NSSize(width: 760, height: 400)

@MainActor
@Suite(.serialized)
struct EditCardSnapshotTests {
  @Test func theCardShowsTheHunkAsTheCoreNumberedIt() throws {
    guard case .diff(let hunks) = try editCard().detail else {
      Issue.record("an edit's card shows its diff")
      return
    }
    #expect(hunks.map(\.header) == ["@@ -2,5 +2,6 @@"])
    let lines = hunks.flatMap(\.lines)
    #expect(lines.map(\.number) == ["2", "3", "4", "5", "5", "6", "7"])
    #expect(
      lines.map(\.kind) == [.context, .context, .context, .removed, .added, .added, .context])
    #expect(lines[4].runs.map(\.text).joined() == "    let ms = 100 * u64::from(attempt);")
  }

  /// Code runs take the session theme's light or dark variant (A95), so the two differ in more
  /// than the card's surface.
  @Test(arguments: [false, true])
  func anOpenedEditCard(dark: Bool) throws {
    let host = try opened(dark: dark)
    defer { host.close() }
    try match(host, dark: dark)
  }

  /// A window that turns dark redraws the runs in the dark variant with no new state (A95).
  @Test func aCardFollowsItsWindowIntoTheDarkAppearance() throws {
    let host = try opened(dark: false)
    defer { host.close() }
    host.window.appearance = NSAppearance(named: .darkAqua)
    host.window.backgroundColor = .black
    RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.05))
    host.flush()
    try match(host, dark: true)
  }

  private func opened(dark: Bool) throws -> Host {
    let host = Host([], size: size, dark: dark)
    host.hosting.rootView = AnyView(
      ToolCard(try editCard(), isExpanded: true)
        .padding(Space.l)
        .environment(\.coxAppearance, Appearance(material: .solid)))
    host.window.setContentSize(NSSize(width: size.width, height: host.hosting.fittingSize.height))
    // No transcript text is left to settle; one turn lets SwiftUI place the card.
    RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.05))
    host.flush()
    return host
  }

  private func match(_ host: Host, dark: Bool) throws {
    // Anti-aliasing differs slightly between machines; a real change moves far more pixels.
    assertSnapshot(
      of: try host.image(), as: .image(precision: 0.995, perceptualPrecision: 0.98),
      named: dark ? "dark-solid" : "light-solid", testName: "anOpenedEditCard")
  }
}
