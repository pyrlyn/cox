// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The `Composer` organism's and `CompletionList`'s check (T37.24, DS§6.3–§6.4): a snapshot per
// state × light/dark × Solid/Frosted — empty, a file mentioned with more files offered,
// commands offered, a shell line with a prompt queued, an image and a file attached, the
// mode and model chips (T37.24.7) with think off, and with think on (T37.24.10), and with the
// provider's reason under the composer (T60.5).

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct ComposerSnapshotTests {
  @Test(arguments: Variant.all) func composer(_ variant: Variant) throws {
    let states = [
      ("empty", PreviewState.composerEmpty), ("mention", PreviewState.composerMention),
      ("commands", PreviewState.composerCommands), ("shell-queued", PreviewState.composerShell),
      ("attachments", PreviewState.composerAttachments), ("status", PreviewState.composerStatus),
      ("think", PreviewState.composerThink), ("not-ready", PreviewState.composerNotReady),
    ]
    for (look, state) in states {
      try assertCoxSnapshot(
        ComposerSample(state: state).fixedSize(), variant, named: "\(look).\(variant.name)")
    }
  }

  @Test(arguments: Variant.all) func completionList(_ variant: Variant) throws {
    let lists = [
      ("files", PreviewState.fileCompletion), ("commands", PreviewState.commandCompletion),
    ]
    for (look, list) in lists {
      try assertCoxSnapshot(
        PreviewPane { CompletionList(state: list) { _ in }.fixedSize() }, variant,
        named: "\(look).\(variant.name)")
    }
  }
}

/// The mode chip's menu (T60.6, DT§5.3): which choices act at once and which ask first.
@Suite struct ModeMenuTests {
  @Test func everyModeOtherThanBypassIsPickedAtOnce() {
    for mode in [SessionMode.ask, .plan, .auto] {
      #expect(ModeMenu.request(mode, from: .ask == mode ? .plan : .ask) == .pick)
    }
  }

  @Test func bypassAsksForConfirmationFirst() {
    for from in [SessionMode.ask, .plan, .auto] {
      #expect(ModeMenu.request(.bypass, from: from) == .confirm)
    }
  }

  @Test func theModeInForceAsksForNothing() {
    for mode in SessionMode.allCases { #expect(ModeMenu.request(mode, from: mode) == .none) }
  }
}
