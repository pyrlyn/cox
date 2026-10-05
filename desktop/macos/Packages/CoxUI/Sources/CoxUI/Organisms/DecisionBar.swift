// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `DecisionBar` (DS§6.4 row `DecisionBar`, the mockup's `.pinned`, DT§5.2 Approval; T37.27.5):
// the approval or question a turn waits on, pinned above the composer while its card stays in
// the transcript, with ⌘⏎ Allow and ⌘⌫ Deny. Separate from `ApprovalCard` because the bar is one
// line that names the call and repeats the buttons, not the whole card, and because it owns
// the window's shortcuts for the decision.

import AppKit
import SwiftUI

/// A symbol in the kind's colour, what waits, then the buttons, on a readable face tinted
/// `status.warning.soft` (an approval) or `accent.soft` (a question), `radius.xl`. A question's
/// offered answers show as buttons while they fit on the line.
public struct DecisionBar: View {
  public enum Content: Equatable, Sendable {
    /// A call waiting to run: the command, or the tool and what it touches.
    case approval(String)
    /// A question and the answers offered, which may be none.
    case question(String, options: [String])
  }

  public enum Choice: Equatable, Sendable {
    case decide(ApprovalCard.Action)
    case answer(String)
  }

  let content: Content
  let choose: (Choice) -> Void

  public init(_ content: Content, choose: @escaping (Choice) -> Void) {
    self.content = content
    self.choose = choose
  }

  public var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.xl, style: .continuous)
    let tint = isApproval ? Color(.statusWarningSoft) : Color(.accentSoft)
    let keys: ((NSEvent) -> Bool)? = isApproval ? { take($0) } : nil
    bar
      .padding(.vertical, Space.m)
      .padding(.horizontal, Space.l)
      .frame(maxWidth: Size.readingWidth)
      .background { shape.fill(tint) }
      .glassPane(shape, role: .readable)
      .hairline(in: shape)
      .background(WindowKeys(handle: keys))
      .accessibilityElement(children: .contain)
      .accessibilityLabel("Waiting for you")
  }

  @ViewBuilder private var bar: some View {
    switch content {
    case .approval(let subject):
      approvalRow(subject)
    case .question(let question, let options):
      // The offered answers while the line holds them; else the question alone.
      ViewThatFits(in: .horizontal) {
        questionRow(question, options: options)
        questionRow(question, options: [])
      }
    }
  }

  private var isApproval: Bool { if case .approval = content { true } else { false } }

  private func approvalRow(_ subject: String) -> some View {
    row(symbol: "exclamationmark.triangle", tint: Color(.statusWarning)) {
      line("Waiting for you:", subject, mono: true)
    } actions: {
      Button("Allow") { choose(.decide(.allow)) }
        .buttonStyle(CoxButtonStyle(.primary, size: .small)).help("Allow (⌘⏎)")
      Button("For session") { choose(.decide(.allowForSession)) }
        .buttonStyle(CoxButtonStyle(.secondary, size: .small)).help("Allow for session")
      Button("Deny") { choose(.decide(.deny)) }
        .buttonStyle(CoxButtonStyle(.danger, size: .small)).help("Deny (⌘⌫)")
    }
  }

  private func questionRow(_ question: String, options: [String]) -> some View {
    row(symbol: "questionmark.circle", tint: Color(.accent)) {
      line("Question from cox:", question, mono: false)
    } actions: {
      ForEach(options, id: \.self) { option in
        Button(option) { choose(.answer(option)) }
          .buttonStyle(CoxButtonStyle(.secondary, size: .small)).fixedSize()
      }
    }
  }

  private func row(
    symbol: String, tint: Color, @ViewBuilder text: () -> some View,
    @ViewBuilder actions: () -> some View
  ) -> some View {
    HStack(spacing: Space.ml) {
      Image(systemName: symbol).symbolStyle(.body).foregroundStyle(tint)
        .accessibilityHidden(true)
      text().frame(maxWidth: .infinity, alignment: .leading)
      actions()
    }
    // A bar without buttons keeps the height of one with them.
    .frame(minHeight: Size.buttonHeightSmall)
  }

  /// `Waiting for you: git push -u origin main`, the subject emphasised; one line, cut in the
  /// middle so both ends of a long command stay.
  private func line(_ label: String, _ subject: String, mono: Bool) -> some View {
    // `monospaced()` keeps the bar's size, the mockup's 12.5 pt `.pinned`.
    let emphasis =
      mono ? Text(subject).monospaced().fontWeight(.semibold) : Text(subject).fontWeight(.medium)
    return Text("\(label) \(emphasis)")
      .textStyle(.compact)
      .foregroundStyle(Color(.textPrimary))
      .lineLimit(1)
      .truncationMode(.middle)
  }

  /// ⌘⏎ (or ⌘ with the keypad's Enter) allows, ⌘⌫ denies; any other key goes on. A held key's
  /// repeats are swallowed rather than sent: one press decides once.
  private func take(_ event: NSEvent) -> Bool {
    guard WindowKeys.holds(event, only: .command) else { return false }
    let decision: ApprovalCard.Action? =
      switch event.charactersIgnoringModifiers {
      case "\r", "\u{3}": .allow
      case "\u{7F}": .deny
      default: nil
      }
    guard let decision else { return false }
    if !event.isARepeat { choose(.decide(decision)) }
    return true
  }
}

#Preview("approval") { PreviewMatrix { DecisionBarSample(PreviewState.decisionApproval) } }
#Preview("question") { PreviewMatrix { DecisionBarSample(PreviewState.decisionQuestion) } }
#Preview("long question") {
  PreviewMatrix { DecisionBarSample(PreviewState.decisionLongQuestion) }
}
