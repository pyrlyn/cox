// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// What a hovered prompt's actions do (T37.23.9, DT§5.2): Copy puts the prompt's text on the
// pasteboard, Edit and resend puts it in the session's composer draft and rewinds the
// conversation to before it (T37.23.18). A hovered prompt's turn number opens its rewind menu
// (T37.48, Figma frame 14), whose scopes and fork reach the session's store. Here because the
// transcript's actions meet the composer's store only in this package; CoxUI draws the strip
// and CoxTranscriptText shows it on hover.

import AppKit
import CoxClient
import CoxModel
import CoxTranscriptText
import CoxUI
import SwiftUI

extension TranscriptView {
  /// The transcript with a prompt's Edit and resend filling `composer`'s draft; without one a
  /// prompt offers Copy only.
  public func composer(_ composer: ComposerStore?) -> Self {
    var view = self
    view.composer = composer
    return view
  }

  /// Gives `text`'s hovered prompts their actions, reaching this view's composer; again on
  /// every update, so a composer given later is the one Edit and resend fills.
  func offerPromptActions(on text: TranscriptTextView, _ shared: SharedAppearance) {
    let acting = PromptActing(composer: composer, store: store)
    text.cards.promptActions = { block in
      AnyView(CardAppearance(shared: shared) { acting.actions(for: block) })
    }
    text.cards.promptGutter = { block, open in
      AnyView(CardAppearance(shared: shared) { acting.gutter(for: block, open: open) })
    }
    text.cards.promptMenu = { block, close in
      AnyView(CardAppearance(shared: shared) { PromptRewindMenu(block, acting, close: close) })
    }
  }
}

/// A prompt's actions and where they reach.
@MainActor
struct PromptActing {
  let composer: ComposerStore?
  /// The session a rewind or a fork goes to.
  var store: SessionStore?
  /// Where Copy writes; tests pass a private one.
  var pasteboard = NSPasteboard.general

  /// What a hovered prompt offers: Edit and resend only with a composer to fill.
  var offered: [PromptActions.Action] { composer == nil ? [.copy] : PromptActions.Action.allCases }

  func actions(for block: Block) -> PromptActions {
    PromptActions(offered) { perform($0, on: block) }
  }

  /// Copy gives the prompt as shown, without its tiles (T37.23.4); Edit and resend replaces the
  /// draft with it, to change and send again, and rewinds the conversation to before it.
  @discardableResult
  func perform(_ action: PromptActions.Action, on block: Block) -> Task<Void, Never>? {
    guard case .user(let text, _) = block.kind else { return nil }
    switch action {
    case .copy:
      pasteboard.clearContents()
      pasteboard.setString(text, forType: .string)
      return nil
    case .edit:
      return composer?.resend(block)
    }
  }
}

extension PromptActing {
  /// The hovered prompt's number, marked as the turn a rewind goes back to before, opening its
  /// menu.
  func gutter(for block: Block, open: @escaping () -> Void) -> TurnGutter {
    TurnGutter(turn: block.turn, isMarked: true, open: open)
  }

  /// A scope rewinds code, the conversation or both to before the turn; Fork starts a child
  /// session from before it. A failure shows where the composer reports its own.
  @discardableResult
  func perform(_ intent: RewindMenu.Intent) -> Task<Void, Never>? {
    guard let store else { return nil }
    return Task { [composer] in
      do {
        switch intent {
        case .rewind(let turn, let code, let conversation):
          try await store.rewind(toTurn: turn, code: code, conversation: conversation)
        case .fork(let turn):
          try await store.fork(beforeTurn: turn)
        }
      } catch {
        composer?.report(error)
      }
    }
  }
}

/// A prompt's rewind menu, its restored-file count arriving after it opens (T37.46).
struct PromptRewindMenu: View {
  let acting: PromptActing
  let close: () -> Void
  @State private var state: RewindMenu.State

  init(_ block: Block, _ acting: PromptActing, close: @escaping () -> Void) {
    (self.acting, self.close) = (acting, close)
    _state = State(initialValue: RewindMenu.State(turn: block.turn))
  }

  var body: some View {
    RewindMenu(state: state) { intent in
      close()
      acting.perform(intent)
    }
    .task {
      guard let store = acting.store else { return }
      state.restoredFiles = await store.rewindMenu(turn: state.turn).restoredFiles
    }
  }
}
