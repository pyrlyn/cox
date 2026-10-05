// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `SessionComposer` (DT§5.3, DS§6.4 row `Composer`): CoxUI's `Composer` over a session's
// `ComposerStore` — the store's draft, rows and think toggle, and the session's token meter, mode
// and model, copied into the organism's value, and each of its intents handed to the store. Here,
// beside `TranscriptView`, because this package is where CoxUI and CoxModel meet; CoxUI stays
// free of the stores and the store free of views.

import CoxClient
import CoxModel
import CoxUI
import Foundation
import SwiftUI

/// The composer under a session's transcript. The paperclip opens the system file picker. While
/// the session waits on an approval or question, its `DecisionBar` sits above the composer.
public struct SessionComposer: View {
  let store: ComposerStore
  @State private var isPicking = false
  @State private var isTokensOpen = false

  public init(store: ComposerStore) {
    self.store = store
  }

  public var body: some View {
    VStack(spacing: Space.m) {
      // The approval or question the turn waits on, pinned while its card stays in the
      // transcript (T37.27.5).
      if let waiting = store.session.waiting {
        DecisionBar(waiting.bar) { decide($0.intent(call: waiting.call)) }
      }
      composer
    }
    // Off the column's edges, as the mockup's `.composer` margin keeps it.
    .padding(.horizontal, Space.xl)
    .padding(.bottom, Space.composerBottom)
  }

  /// An Allow, Deny or answer from the bar, sent as the card sends it; a failure shows as the
  /// composer's.
  private func decide(_ intent: Intent) {
    Task {
      do {
        _ = try await store.session.send(intent)
      } catch {
        store.report(error)
      }
    }
  }

  private var composer: some View {
    Composer(state: state, send: handle)
      .fileImporter(
        isPresented: $isPicking, allowedContentTypes: [.item], allowsMultipleSelection: true
      ) {
        switch $0 {
        case .success(let urls): Task { await store.attach(urls) }
        case .failure(let error): store.report(error)
        }
      }
  }

  private var state: Composer.State {
    var state = Composer.State()
    state.text = store.text
    state.selectedRange = store.selectedRange
    state.isShell = store.isShell
    state.shareOutput = store.shareOutput
    state.mentions = store.mentions.map {
      Composer.Mention(
        id: $0, label: URL(fileURLWithPath: String($0.dropFirst())).lastPathComponent)
    }
    if let first = store.completions.first {
      state.completion = CompletionList.State(
        title: first.insert.hasPrefix("@") ? "Files" : "Commands",
        rows: store.completions.map {
          CompletionList.Row(id: $0.insert, title: $0.insert, detail: $0.detail)
        },
        selection: store.selection)
    }
    state.attachments = store.attachments.enumerated().map { index, file in
      // Only an image shows its picture; the bytes are decoded from what will be sent.
      let image = file.mediaType.hasPrefix("image/") ? Data(base64Encoded: file.dataB64) : nil
      return Composer.Attachment(id: String(index), name: file.name, image: image)
    }
    state.canSend = store.canSend
    state.isRunning = store.isRunning
    state.queued = store.queued
    state.isRecalling = store.isRecalling
    state.failure = store.failure
    state.mode = store.mode.map(SessionMode.init)
    state.model = store.model
    state.think = store.think
    if let usage = store.session.usage {
      state.meter = TokenMeter.State(usage, isRunning: store.isRunning)
      state.tokens = isTokensOpen ? TokenPopover.State(usage, isRunning: store.isRunning) : nil
    }
    return state
  }

  private func handle(_ intent: Composer.Intent) {
    switch intent {
    case .attach: isPicking = true
    case .toggleTokens: isTokensOpen.toggle()
    case .drop(let urls): Task { await store.attach(urls) }
    case .pasteImage(let png): store.attach(png, name: "Pasted image.png", type: .png)
    case .removeAttachment(let id): if let index = Int(id) { store.removeAttachment(at: index) }
    case .recall(let step): store.recall(step)
    case .select(let range): store.select(range)
    case .cycleMode, .toggleThink: chip(intent)
    default: draft(intent)
    }
  }

  /// The chips that ask for something: the next mode, or think for the next turn.
  private func chip(_ intent: Composer.Intent) {
    switch intent {
    case .cycleMode: Task { await store.cycleMode() }
    case .toggleThink: store.toggleThink()
    default: break
    }
  }

  /// The intents about the text, its rows and shell mode.
  private func draft(_ intent: Composer.Intent) {
    switch intent {
    case .edit(let text): store.edit(text)
    case .submit: Task { await store.submit() }
    case .submitNow: Task { await store.submitNow() }
    case .moveSelection(let step): store.moveSelection(by: step)
    case .pick(let index): store.pick(index)
    case .dismissCompletion: store.dismissCompletion()
    case .removeMention(let insert): store.removeMention(insert)
    case .leaveShell: store.leaveShell()
    case .shareOutput(let share): store.shareOutput = share
    default: break
    }
  }
}

extension SessionMode {
  /// The core's mode as the composer and the toolbar name it: `default` is Ask.
  public init(_ mode: PermissionMode) {
    switch mode {
    case .default: self = .ask
    case .plan: self = .plan
    case .auto: self = .auto
    case .bypass: self = .bypass
    }
  }
}

extension PermissionMode {
  /// The toolbar's mode as the core names it: Ask is `default`.
  public init(_ mode: SessionMode) {
    switch mode {
    case .ask: self = .default
    case .plan: self = .plan
    case .auto: self = .auto
    case .bypass: self = .bypass
    }
  }
}
