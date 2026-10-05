// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `QuestionCard` (DS§6.4 row `QuestionCard`, the mockup's question `.appr`, DT§5.2 Question): the
// agent asks the person something — the question, one button per offered answer and a field
// for their own — and once answered, the one line "You answered: …". Separate from
// `ApprovalCard`, whose decision is fixed, because the answer here is free text.

import SwiftUI

/// Pending: an accent question symbol and who asks, the question in `font.transcript`, a button
/// per option, then a field and Answer on the action row, in the shared `DecisionFrame`.
/// Answered: a `NoticeRow`. The typed draft is the card's own until it is sent.
public struct QuestionCard: View {
  public struct Content: Equatable, Sendable {
    public var question: String
    /// The answers the agent offers; may be empty.
    public var options: [String]
    /// The subagent that asks; `nil` for the main agent.
    public var source: String?
    /// Set once answered: the card shrinks to it.
    public var answer: String?

    public init(
      question: String, options: [String] = [], source: String? = nil, answer: String? = nil
    ) {
      (self.question, self.options, self.source, self.answer) = (question, options, source, answer)
    }
  }

  let content: Content
  let answer: (String) -> Void
  @State private var draft = ""

  public init(_ content: Content, answer: @escaping (String) -> Void) {
    self.content = content
    self.answer = answer
  }

  public var body: some View {
    if let answered = content.answer {
      NoticeRow("You answered: \(answered)", symbol: "checkmark.circle")
    } else {
      DecisionFrame(edge: Color(.accent)) {
        DecisionTitle(
          "Question from \(content.source ?? "cox")", symbol: "questionmark.circle",
          tint: Color(.accent))
        Text(content.question)
          .textStyle(.transcript)
          .foregroundStyle(Color(.textPrimary))
          .textSelection(.enabled)
          .fixedSize(horizontal: false, vertical: true)
        if !content.options.isEmpty {
          VStack(alignment: .leading, spacing: Space.s) {
            ForEach(content.options, id: \.self) { option in
              Button(option) { answer(option) }.buttonStyle(
                CoxButtonStyle(.secondary, size: .small))
            }
          }
        }
      } actions: {
        TextField(
          "Answer", text: $draft,
          prompt: Text("Or type your own answer…").foregroundStyle(Color(.textTertiary))
        )
        .textFieldStyle(.plain)
        .textStyle(.body)
        .foregroundStyle(Color(.textPrimary))
        .lineLimit(1)
        .onSubmit(send)
        .padding(.horizontal, Space.ml)
        .frame(height: Size.buttonHeightSmall)
        .insetWell(Color(.surfaceWindow), cornerRadius: Radius.m)
        Button("Answer", action: send)
          .buttonStyle(CoxButtonStyle(.primary, size: .small))
          .disabled(trimmed.isEmpty)
      }
    }
  }

  private var trimmed: String { draft.trimmingCharacters(in: .whitespacesAndNewlines) }

  private func send() {
    guard !trimmed.isEmpty else { return }
    answer(trimmed)
    draft = ""
  }
}

#Preview("options") { PreviewMatrix { QuestionCardSample(PreviewState.questionOptions) } }
#Preview("free text") { PreviewMatrix { QuestionCardSample(PreviewState.questionOpen) } }
#Preview("answered") { PreviewMatrix { QuestionCardSample(PreviewState.questionAnswered) } }
