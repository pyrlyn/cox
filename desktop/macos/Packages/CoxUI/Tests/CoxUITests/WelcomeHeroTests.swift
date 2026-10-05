// Figma sync checks for frames 22-empty-session, 01-main-session-streaming and
// 14-rewind-edit-resend: the app icon, the welcome hero, the turn gutter and the rewind menu per
// light/dark × Solid/Frosted cell, and what the hero and the menu report.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct WelcomeHeroSnapshotTests {
  @Test(arguments: Variant.all) func appIcon(_ variant: Variant) throws {
    try assertCoxSnapshot(PreviewPane { AppIcon() }, variant, named: variant.name)
  }

  @Test(arguments: Variant.all) func welcomeHero(_ variant: Variant) throws {
    try assertCoxSnapshot(
      WelcomeHeroSample(state: PreviewState.welcome), variant, named: variant.name)
  }

  @Test(arguments: Variant.all) func turnGutter(_ variant: Variant) throws {
    let sample = PreviewPane {
      VStack(spacing: Space.m) {
        TurnGutter(turn: 1)
        TurnGutter(turn: 2, isMarked: true) {}
      }
    }
    try assertCoxSnapshot(sample, variant, named: variant.name)
  }

  @Test(arguments: Variant.all) func rewindMenu(_ variant: Variant) throws {
    try assertCoxSnapshot(
      PreviewPane { RewindMenu(state: PreviewState.rewindMenu) { _ in } }, variant,
      named: variant.name)
  }
}

@MainActor
@Suite struct WelcomeHeroTests {
  @Test func eachScopeRewindsWhatItsTitleSays() {
    #expect(
      RewindMenu.Scope.allCases.map { RewindMenu.intent($0, turn: 2) } == [
        .rewind(turn: 2, code: true, conversation: true),
        .rewind(turn: 2, code: true, conversation: false),
        .rewind(turn: 2, code: false, conversation: true),
      ])
  }

  @Test func theRestoredCountReadsAsTheFrameWordsIt() {
    let menu = { (count: Int?) in RewindMenu(state: .init(turn: 2, restoredFiles: count)) { _ in } }
    #expect(menu(2).hint(.codeAndConversation) == "2 files restored")
    #expect(menu(1).hint(.codeAndConversation) == "1 file restored")
    #expect(menu(nil).hint(.codeAndConversation) == nil)
    #expect(menu(nil).hint(.code) == "keep the chat")
    #expect(menu(nil).hint(.conversation) == "keep the files")
  }
}
