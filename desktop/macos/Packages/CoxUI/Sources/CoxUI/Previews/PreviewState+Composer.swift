// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the composer's molecules (T37.21.7): the chips of a message
// about to be sent, as mockup screens 1 and 6 show them. Separate from `PreviewState.swift` so
// molecules built in parallel add their fixtures without editing one file.

import SwiftUI

extension PreviewState {
  /// A chip's label per kind.
  static func chip(_ kind: ComposerChip.Kind) -> String {
    switch kind {
    case .mention: "crates/cox-provider/src/retry.rs"
    case .attachment: fileName
    case .command: "/review"
    case .shell: "Shell · share output"
    case .queued: "Queued · 1"
    case .model: modelChip
    case .mode(let mode): mode.title
    case .think: thinkChip
    }
  }

  static let modeShortcut = "⇧⇥"
  /// The model and effort as the composer's chip shows them.
  static let modelChip = "claude-sonnet-5 · high"
  /// The think toggle's label (A103).
  static let thinkChip = "Think"
}

/// A removable chip of `kind`.
struct ComposerChipSample: View {
  let kind: ComposerChip.Kind

  var body: some View {
    ComposerChip(PreviewState.chip(kind), kind: kind) {}
  }

  /// The mode chip with the shortcut that cycles it.
  static var shortcut: some View {
    ComposerChip(
      SessionMode.plan.title, kind: .mode(.plan), shortcut: PreviewState.modeShortcut)
  }
}

// The `Composer` organism's fixtures (T37.24): mockup screens 1 (empty, attachments), 5 (a file
// picked, more files offered), 6 (commands offered) and 7 (shell mode with a prompt queued).
extension PreviewState {
  static let composerEmpty = Composer.State()

  static var composerMention: Composer.State {
    var state = Composer.State()
    state.text =
      "Now apply the same backoff to MCP reconnects in @crates/cox-mcp/src/client.rs and @mcp/au"
    state.mentions = [Composer.Mention(id: "@crates/cox-mcp/src/client.rs", label: "client.rs")]
    state.completion = fileCompletion
    state.canSend = true
    return state
  }

  static var composerCommands: Composer.State {
    var state = Composer.State()
    state.text = "/co"
    state.completion = commandCompletion
    state.canSend = true
    return state
  }

  static var composerShell: Composer.State {
    var state = Composer.State()
    state.text = "git log --oneline -5 wt/retry-jitter"
    state.isShell = true
    state.isRunning = true
    state.queued = 1
    state.canSend = true
    return state
  }

  /// A pasted screenshot and a dropped log, with a line about them (mockup screen 1's `atts`).
  @MainActor static var composerAttachments: Composer.State {
    var state = Composer.State()
    state.text = "Why does the retry loop give up here?"
    let picture = ImageRenderer(
      content: PreviewBackdrop().frame(width: Size.windowMinWidth, height: Size.windowMinHeight)
    ).nsImage?.tiffRepresentation
    state.attachments = [
      Composer.Attachment(id: "0", name: imageName, image: picture),
      Composer.Attachment(id: "1", name: fileName),
    ]
    state.canSend = true
    return state
  }

  /// The status chips under an empty draft (T37.24.7, mockup screen 1): Plan, the model and
  /// its effort.
  static var composerStatus: Composer.State {
    var state = Composer.State()
    state.mode = .plan
    state.model = modelChip
    return state
  }

  /// The status chips with think on for the next turn (T37.24.10, A103).
  static var composerThink: Composer.State {
    var state = composerStatus
    state.think = true
    return state
  }

  /// A send the store refused, the draft and its file still there (T37.24.9).
  static var composerFailure: Composer.State {
    var state = Composer.State()
    state.text = "Compare this log with the last run"
    state.attachments = [Composer.Attachment(id: "0", name: fileName)]
    state.isRunning = true
    state.canSend = true
    state.failure = "Attachments cannot wait in the queue; ⌘⏎ sends them now."
    return state
  }

  static let fileCompletion = CompletionList.State(
    title: "Files",
    rows: [
      "crates/cox-mcp/src/auth.rs", "crates/cox-mcp/tests/oauth_flow.rs",
      "crates/cox-mcp/src/auth/pkce.rs",
    ].map { CompletionList.Row(id: "@\($0)", title: "@\($0)", detail: $0) })

  static let commandCompletion = CompletionList.State(
    title: "Commands",
    rows: [
      ("/compact", "/compact [focus]"), ("/cost", "/cost"), ("/context", "/context"),
    ].map { CompletionList.Row(id: $0.0, title: $0.0, detail: $0.1) })

  /// Room above a composer for the completion rows it floats over it.
  static let completionRoom: CGFloat = 200
}

/// A composer at the reading width, with room above it for its completion rows.
struct ComposerSample: View {
  let state: Composer.State

  var body: some View {
    Composer(state: state) { _ in }
      .frame(width: Size.readingWidth)
      .padding(.top, state.completion == nil ? 0 : PreviewState.completionRoom)
      .padding(.top, state.tokens == nil ? 0 : PreviewState.tokensRoom)
  }
}
