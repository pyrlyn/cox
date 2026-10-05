// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ThinkingDisclosure` (DS§6.3 row `ThinkingDisclosure`, the mockup's `.think` and
// `.think-body`): the model's reasoning in a turn, folded to one line saying how long it
// thought. Separate so the reasoning stays out of the way of the answer and opens the same way
// in every turn.

import SwiftUI

/// A chevron and the summary in `font.caption`; open, the reasoning in italics beside a
/// hairline rule. Open or closed is the view's own state (DS§9).
struct ThinkingDisclosure: View {
  /// What the core wrote for the block, `Thought for 12 s`.
  let summary: String
  /// The reasoning as the model streamed it.
  let text: String
  @State private var isExpanded: Bool

  init(_ summary: String, text: String, isExpanded: Bool = false) {
    self.summary = summary
    self.text = text
    self._isExpanded = State(initialValue: isExpanded)
  }

  var body: some View {
    VStack(alignment: .leading, spacing: Space.xs) {
      ThinkingHeader(summary, isExpanded: isExpanded) { isExpanded.toggle() }
      if isExpanded {
        Text(text)
          .italic()
          .textStyle(.caption)
          .foregroundStyle(Color(.textSecondary))
          .textSelection(.enabled)
          .padding(.vertical, Space.xxs)
          .padding(.leading, Space.l)
          // The mockup's 2 px rule is drawn as the one line width the tokens have.
          .hairline(.leading)
          .coxTransition(.opacity)
      }
    }
    .animation(.cox(Motion.durationBase), value: isExpanded)
  }
}

/// The disclosure's one-line row, the chevron and the summary, on its own so the transcript's
/// text view (T37.23.4) can show a thought's fold above reasoning it draws as text; the caller
/// owns whether it is open.
public struct ThinkingHeader: View {
  let summary: String
  let isExpanded: Bool
  let action: () -> Void

  public init(_ summary: String, isExpanded: Bool, action: @escaping () -> Void) {
    self.summary = summary
    self.isExpanded = isExpanded
    self.action = action
  }

  public var body: some View {
    Button(action: action) {
      HStack(spacing: Space.s) {
        Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
          .symbolStyle(.label)
        Text(summary).textStyle(.caption)
      }
      .foregroundStyle(Color(.textSecondary))
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .accessibilityValue(isExpanded ? "expanded" : "collapsed")
  }
}

#Preview("collapsed") { PreviewMatrix { ThinkingDisclosureSample(isExpanded: false) } }
#Preview("expanded") { PreviewMatrix { ThinkingDisclosureSample(isExpanded: true) } }
