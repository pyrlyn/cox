// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Candidate A: Textual 0.5.0 (research.md §9.5.10), in the two shapes it allows.
// `TextualBlocksTranscript` is the shape T37.23 needs: a lazy list with one Textual
// view per block and SwiftUI tool cards. `TextualDocumentTranscript` is Textual's
// best case for cross-block drag: the whole transcript as one `StructuredText`
// (tool cards reduced to block quotes, code wrapped so it stays in the document's
// selection instead of its own scroll region).

import SwiftUI
import Textual

public struct TextualBlocksTranscript: View {
  let blocks: [Block]

  public init(blocks: [Block]) { self.blocks = blocks }

  public var body: some View {
    ScrollView {
      LazyVStack(alignment: .leading, spacing: 10) {
        ForEach(blocks) { TextualBlockView(block: $0) }
      }
      .padding(16)
    }
    .textual.textSelection(.enabled)
  }
}

struct TextualBlockView: View {
  let block: Block

  var body: some View {
    switch block.kind {
    case .prose, .code, .diff:
      StructuredText(markdown: block.markdown)
        .frame(maxWidth: .infinity, alignment: .leading)
    case .tool:
      VStack(alignment: .leading, spacing: 4) {
        Text(block.tool ?? "tool").bold()
        InlineText(markdown: "`\(block.body)`")
      }
      .padding(8)
      .frame(maxWidth: .infinity, alignment: .leading)
      .overlay(RoundedRectangle(cornerRadius: 8).stroke(.separator))
    }
  }
}

public struct TextualDocumentTranscript: View {
  let markdown: String

  public init(blocks: [Block]) { self.markdown = Fixture.markdown(blocks) }

  public var body: some View {
    ScrollView {
      StructuredText(markdown: markdown)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(16)
    }
    .textual.textSelection(.enabled)
    .textual.overflowMode(.wrap)
  }
}
