// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The rewind timeline's check (T37.28.1, DT§5.4, DS§6.4): the checkpoints with one lifted per
// light/dark × Solid/Frosted cell, and empty; the scope each row action reports.

import Testing

@testable import CoxUI

@MainActor
@Suite struct RewindTimelineSnapshotTests {
  @Test(arguments: Variant.all) func rewindTimeline(_ variant: Variant) throws {
    try assertCoxSnapshot(
      RewindTimelineSample(state: PreviewState.rewind), variant, named: variant.name)
  }

  @Test func emptyRewindTimeline() throws {
    try assertCoxSnapshot(
      RewindTimelineSample(state: .init()), Variant.all[0], named: Variant.all[0].name)
  }
}

@MainActor
@Suite struct RewindTimelineTests {
  /// Records what the timeline sends.
  final class Log {
    var intents: [RewindTimeline.Intent] = []
  }

  @Test func aCheckpointRewindsCodeConversationOrBoth() {
    let log = Log()
    let actions = RewindTimeline.actions("2") { log.intents.append($0) }
    for action in actions { action.perform() }
    #expect(
      actions.map(\.title) == [
        "Restore code", "Restore conversation", "Restore code and conversation",
      ])
    #expect(
      log.intents == [
        .rewind(checkpoint: "2", code: true, conversation: false),
        .rewind(checkpoint: "2", code: false, conversation: true),
        .rewind(checkpoint: "2", code: true, conversation: true),
      ])
  }
}
