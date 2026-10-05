// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for `ApprovalCard` and `QuestionCard` (T37.27): the mockup's push that
// waits on the person, a subagent's risky call, a split line with its grant and Edit… (T37.27.6),
// the two decided lines, and the mockup's retry
// question with options, without them, and answered. Separate so the organisms' fixtures do
// not edit the shared file.

import SwiftUI

extension PreviewState {
  /// The mockup's `03-approval-required` card.
  static let approvalPending = ApprovalCard.Content(
    title: "Run this command?", command: "git push -u origin wt/retry-jitter",
    reason: "matches ask rule Bash(git push:*)")

  /// A subagent's destructive call.
  static let approvalRisky = ApprovalCard.Content(
    title: "Run this command?", command: "rm -rf target/debug/incremental",
    reason: "risk: destructive", source: "reviewer",
    risk: ToolHeader.Risk(text: "destructive", level: .high))

  /// A split command line: Allow for session grants each of its commands, and Edit… is offered.
  static let approvalGrant = ApprovalCard.Content(
    title: "Run this command?", command: "cargo fmt && git push -u origin wt/retry-jitter",
    reason: "matches ask rule Bash(git push:*)",
    grant: "bash: cargo fmt · git push -u origin wt/retry-jitter",
    input: #"{"command": "cargo fmt && git push -u origin wt/retry-jitter"}"#)

  static let approvalAllowed = ApprovalCard.Content(
    title: "Run this command?", command: "git push -u origin wt/retry-jitter",
    reason: "matches ask rule Bash(git push:*)",
    outcome: .init(text: "Allowed by you · for session", isAllowed: true))

  static let approvalDenied = ApprovalCard.Content(
    title: "Run this command?", command: "git push -u origin wt/retry-jitter",
    reason: "matches ask rule Bash(git push:*)",
    outcome: .init(text: "Denied by you", isAllowed: false))

  /// The mockup's `04-question-ask-user` card.
  static let questionOptions = QuestionCard.Content(
    question: "When a 429 arrives with retry-after: 120, which should win?",
    options: [
      "Respect retry-after, even above the 30 s cap", "Always cap at 30 s",
      "Ask me each time it exceeds the cap",
    ])

  static let questionOpen = QuestionCard.Content(
    question: "Which branch should the fix land on?", source: "planner")

  static let questionAnswered = QuestionCard.Content(
    question: "When a 429 arrives with retry-after: 120, which should win?",
    answer: "Always cap at 30 s")
}

/// An approval card across the reading column.
struct ApprovalCardSample: View {
  let content: ApprovalCard.Content

  init(_ content: ApprovalCard.Content) { self.content = content }

  var body: some View {
    ApprovalCard(content, act: { _ in }, edit: { _ in }).frame(width: Size.readingWidth)
  }
}

/// A question card across the reading column.
struct QuestionCardSample: View {
  let content: QuestionCard.Content

  init(_ content: QuestionCard.Content) { self.content = content }

  var body: some View { QuestionCard(content) { _ in }.frame(width: Size.readingWidth) }
}
