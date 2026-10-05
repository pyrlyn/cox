// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `CodeBlockView` (DS§6.3 row `CodeBlockView`, the mockup's `.codeblock`): a fenced code block in
// a message — its language, a copy button and the highlighted code. Separate so every block of
// code the model writes reads the same, and the code arrives already highlighted (`CodeRun`).

import SwiftUI

/// A header with the language in `text.secondary` and an icon-only copy button over a hairline,
/// then the code in `font.mono.code`, scrolling sideways rather than wrapping, all on
/// `surface.code` in a `radius.l` card with a hairline rim.
struct CodeBlockView: View {
  /// The fence's language as the core names it, `rust`, or `nil` for none.
  let language: String?
  let lines: [[CodeRun]]
  /// Reports the copy intent; the app puts the block's text on the pasteboard.
  let copy: () -> Void

  init(language: String?, lines: [[CodeRun]], copy: @escaping () -> Void) {
    self.language = language
    self.lines = lines
    self.copy = copy
  }

  var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.l, style: .continuous)
    VStack(alignment: .leading, spacing: 0) {
      HStack(spacing: Space.m) {
        if let language {
          Text(language).textStyle(.footnote).foregroundStyle(Color(.textSecondary))
        }
        Spacer(minLength: 0)
        Button(action: copy) {
          Image(systemName: "doc.on.doc").symbolStyle(.footnote)
        }
        .buttonStyle(CoxButtonStyle(.plain, size: .small))
        .help("Copy code")
        .accessibilityLabel("Copy code")
      }
      .padding(.leading, Space.l)
      .padding(.trailing, Space.xxs)
      .hairline(.bottom)
      ScrollView(.horizontal) {
        Text(CodeRun.attributed(lines))
          .textStyle(.monoCode)
          .foregroundStyle(Color(.textPrimary))
          .textSelection(.enabled)
          .fixedSize()
          .padding(.horizontal, Space.l)
          .padding(.vertical, Space.ml)
      }
      .scrollIndicators(.never)
    }
    .background(Color(.surfaceCode))
    .clipShape(shape)
    .hairline(in: shape)
  }
}

#Preview("language") { PreviewMatrix { CodeBlockSample(language: PreviewState.codeLanguage) } }
#Preview("plain") { PreviewMatrix { CodeBlockSample(language: nil) } }
