// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the turn molecules (T37.21.5, T37.21.6): a user prompt with its
// attachments, a thinking block, the notices, the divider and the meta line, as mockup screen
// 1 shows a turn. Separate from `PreviewState.swift` so molecules built in parallel add their
// fixtures without editing one file.

import SwiftUI

extension PreviewState {
  static let userPrompt =
    "Add jitter to the retry backoff in the provider HTTP client. Full jitter, capped at 30 s. "
    + "Keep the existing tests green and add one for the cap."
  /// A pasted screenshot and a log file.
  @MainActor static let userAttachments: [UserBubble.Attachment] = [
    .init(id: "a1", name: imageName, image: screenshot),
    .init(id: "a2", name: fileName),
  ]

  static let thinkingSummary = "Thought for 12 s"
  static let thinkingText =
    "The delay doubles with no ceiling, so attempt 16 waits over an hour. Cap it first, then "
    + "draw uniformly below the cap so clients that failed together retry apart."

  /// One notice per kind, as the transcript shows them.
  static func notice(_ kind: NoticeRow.Kind) -> String {
    switch kind {
    case .info:
      "This session is driven by Claude Code over the Agent Client Protocol. Its own model, "
        + "auth and billing apply."
    case .warning:
      "Bypass mode is on: tools run without asking. The shell still runs inside the sandbox."
    case .error: "Anthropic returned 529 overloaded. Retrying in 4 s."
    }
  }

  static let dividerLabel = "Compacted · 48.2k → 12.1k"

  static let turnMeta = TurnMeta.Facts(
    model: "Sonnet 5", tokens: "in 48.2k · out 3.1k", cache: "cache 91%", cost: "$0.44",
    duration: "2 m 18 s", stopReason: "end turn")
  /// A turn on the session's model with nothing cached.
  static let turnMetaShort = TurnMeta.Facts(
    tokens: "in 2.4k · out 310", cost: "$0.01", duration: "4 s")
}

/// A prompt, with or without attachments, in a narrow column so it wraps.
struct UserBubbleSample: View {
  let hasAttachments: Bool

  var body: some View {
    UserBubble(
      PreviewState.userPrompt, attachments: hasAttachments ? PreviewState.userAttachments : []
    )
    .frame(width: Size.popoverWidth)
  }
}

/// The thinking block, closed or open, in a narrow column so the reasoning wraps.
struct ThinkingDisclosureSample: View {
  let isExpanded: Bool

  var body: some View {
    ThinkingDisclosure(
      PreviewState.thinkingSummary, text: PreviewState.thinkingText, isExpanded: isExpanded
    )
    .frame(width: Size.popoverWidth, alignment: .leading)
  }
}

/// A notice of `kind` in a narrow column so it wraps.
struct NoticeRowSample: View {
  let kind: NoticeRow.Kind

  var body: some View {
    NoticeRow(PreviewState.notice(kind), kind: kind).frame(width: Size.popoverWidth)
  }
}

/// The divider, labelled or bare, across a narrow column.
struct TurnDividerSample: View {
  let label: String?

  var body: some View {
    TurnDivider(label).frame(width: Size.popoverWidth)
  }
}
