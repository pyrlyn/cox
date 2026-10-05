// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `DiffLineView` (DS§6.3 row `DiffLineView`, the mockup's `.diff .ln`): one line of a diff — its
// line number in a gutter, its sign and its highlighted code on the added, removed or context
// background. Separate so a tool card, the review pane and the changes tab draw a line the same
// way; `DiffHunkView` stacks them.

import SwiftUI

/// The gutter number right-aligned in `text.secondary` (`text.primary` on a tinted gutter; the
/// mockup's tertiary would miss DS§8's 4.5:1 on a light code surface),
/// then `+`, `-` or a space and the code in `font.mono.code`, cut with an ellipsis. A replaced
/// pair's changed words sit on the gutter's stronger `diff.*Gutter` tint (T37.23.11).
public struct DiffLineView: View {
  public enum Kind: CaseIterable, Sendable {
    case context, added, removed
  }

  public struct Line: Equatable, Sendable {
    public var kind: Kind
    /// The line number the core picked (new for added and context, old for removed), `42`.
    public var number: String
    public var runs: [CodeRun]

    public init(kind: Kind, number: String, runs: [CodeRun]) {
      self.kind = kind
      self.number = number
      self.runs = runs
    }
  }

  let line: Line
  /// The widest number in the hunk, so every gutter in it has one width at any text size.
  let widestNumber: String
  /// Review's click on the number (T37.28.4); `nil` where a line takes no comment.
  let comment: (@MainActor () -> Void)?

  init(_ line: Line, widestNumber: String? = nil, comment: (@MainActor () -> Void)? = nil) {
    self.line = line
    self.widestNumber = widestNumber ?? line.number
    self.comment = comment
  }

  /// The sign and the runs; a changed word takes the gutter's tint.
  private var code: AttributedString {
    CodeRun.attributed([[CodeRun("\(line.kind.sign) ")] + line.runs], mark: line.kind.gutter)
  }

  public var body: some View {
    HStack(spacing: 0) {
      if let comment {
        gutter.contentShape(Rectangle()).onTapGesture(perform: comment)
      } else {
        gutter
      }
      Text(code)
        .foregroundStyle(Color(.textPrimary))
        .padding(.leading, Space.m)
        // `font.mono.code`'s 1.55 line height, which one-line text does not get from
        // `lineSpacing`; the gutter stretches to match.
        .padding(.vertical, Space.xxs)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
    .textStyle(.monoCode)
    .lineLimit(1)
    .background(line.kind.background)
    .fixedSize(horizontal: false, vertical: true)
    .accessibilityElement(children: .ignore)
    .accessibilityLabel("\(line.kind.label) \(line.number): \(line.runs.map(\.text).joined())")
    .accessibilityActions {
      if let comment { Button("Comment", action: comment) }
    }
  }

  private var gutter: some View {
    ZStack(alignment: .trailing) {
      Text(widestNumber).hidden()
      Text(line.number)
    }
    .foregroundStyle(line.kind == .context ? Color(.textSecondary) : Color(.textPrimary))
    .padding(.leading, Space.m)
    .padding(.trailing, Space.ml)
    .frame(maxHeight: .infinity)
    .background(line.kind.gutter)
  }
}

extension DiffLineView.Kind {
  var sign: String {
    switch self {
    case .context: " "
    case .added: "+"
    case .removed: "-"
    }
  }

  var background: Color {
    switch self {
    case .context: .clear
    case .added: Color(.diffAdd)
    case .removed: Color(.diffDel)
    }
  }

  var gutter: Color {
    switch self {
    case .context: .clear
    case .added: Color(.diffAddGutter)
    case .removed: Color(.diffDelGutter)
    }
  }

  var label: String {
    switch self {
    case .context: "Line"
    case .added: "Added line"
    case .removed: "Removed line"
    }
  }
}

#Preview("context") { PreviewMatrix { DiffLineSample(kind: .context) } }
#Preview("added") { PreviewMatrix { DiffLineSample(kind: .added) } }
#Preview("removed") { PreviewMatrix { DiffLineSample(kind: .removed) } }
