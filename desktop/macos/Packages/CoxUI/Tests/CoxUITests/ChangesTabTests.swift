// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Changes tab's check (T37.29, DT§5.1, DS§6.4): the inspector on its Changes tab per
// light/dark × Solid/Frosted cell as the mockup shows it, and empty; the intents its rows and
// header report; its files header.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct ChangesTabSnapshotTests {
  @Test(arguments: Variant.all) func changesTab(_ variant: Variant) throws {
    try assertCoxSnapshot(
      ChangesInspectorSample(state: PreviewState.changes), variant, named: variant.name)
  }

  @Test func emptyChangesTab() throws {
    try assertCoxSnapshot(
      ChangesInspectorSample(state: .init()), Variant.all[0], named: Variant.all[0].name)
  }
}

@MainActor
@Suite struct ChangesTabTests {
  /// Records what a tab sends.
  final class Log {
    var intents: [ChangesTab.Intent] = []
  }

  @Test func aFileRowOpensItInReviewOrRevertsIt() {
    let log = Log()
    let actions = ChangesTab.fileActions("src/retry.rs") { log.intents.append($0) }
    for action in actions { action.perform() }
    #expect(actions.map(\.title) == ["Review", "Revert"])
    #expect(log.intents == [.open(path: "src/retry.rs"), .revert(path: "src/retry.rs")])
  }

  @Test func aCheckpointRowRewindsToItsId() {
    let log = Log()
    for action in ChangesTab.checkpointActions("cp-2", send: { log.intents.append($0) }) {
      action.perform()
    }
    #expect(log.intents == [.rewind(checkpoint: "cp-2")])
  }

  @Test func theFilesHeaderCountsTheFiles() {
    #expect(PreviewState.changes.filesTitle == "This session · 3 files")
    let one = ChangesTab.State(files: [PreviewState.changedFiles[0]])
    #expect(one.filesTitle == "This session · 1 file")
  }

  @Test func reviewAnswersTheDesignKey() {
    #expect(ShellShortcut.review.key == KeyboardShortcut("r", modifiers: [.command, .shift]))
    #expect(ShellShortcut.review.help("Review changes") == "Review changes (⌘⇧R)")
  }
}
