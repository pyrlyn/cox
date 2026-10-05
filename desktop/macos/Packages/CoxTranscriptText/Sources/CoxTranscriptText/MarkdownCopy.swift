// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Copy as Markdown (T37.42, DT§5.2): a selection goes to the pasteboard as
// Markdown in block order, next to plain text. Each block's Markdown comes
// from its timeline value, not from the drawn text, so a card copies as its
// summary line however it is drawn (T37.41) and a whole reply as its
// Markdown source, or as the core's writer writes its doc (T58.4.28). Its own
// file because it hooks only AppKit's pasteboard path; the clamp lives in
// `Selection.swift`.

import AppKit
import CoxClient

extension NSPasteboard.PasteboardType {
  /// Markdown's system type.
  public static let markdown = NSPasteboard.PasteboardType("net.daringfireball.markdown")
}

extension TranscriptTextView {
  override public var writablePasteboardTypes: [NSPasteboard.PasteboardType] {
    [.markdown, .string]
  }

  override public func writeSelection(
    to pboard: NSPasteboard, types: [NSPasteboard.PasteboardType]
  ) -> Bool {
    let wanted = types.filter(writablePasteboardTypes.contains)
    guard !wanted.isEmpty else { return super.writeSelection(to: pboard, types: types) }
    let copied = copiedSelection()
    pboard.declareTypes(wanted, owner: nil)
    return wanted.allSatisfy {
      pboard.setString($0 == .markdown ? copied.markdown : copied.plain, forType: $0)
    }
  }

  /// The selection as Markdown and as plain text, blocks in transcript order.
  public func copiedSelection() -> (markdown: String, plain: String) {
    MarkdownCopy.copy(
      selectedRanges.map(\.rangeValue), text: string as NSString, ranges: blockRanges,
      blocks: blocks)
  }
}

enum MarkdownCopy {
  /// Blocks join with a blank line in Markdown and a line break in plain text.
  static func copy(
    _ selection: [NSRange], text: NSString, ranges: BlockRanges, blocks: [BlockID: Block]
  ) -> (markdown: String, plain: String) {
    var markdown: [String] = []
    var plain: [String] = []
    for range in selection where range.length > 0 {
      var index = ranges.index(at: range.location) ?? ranges.count
      while index < ranges.count, ranges.ranges[index].location < NSMaxRange(range) {
        let full = ranges.ranges[index]
        let part = NSIntersectionRange(range, full)
        let kind = blocks[ranges.ids[index]]?.kind
        index += 1
        guard part.length > 0 else { continue }
        if let line = kind.flatMap(card) {
          markdown.append(line)
          plain.append(line)
          continue
        }
        let selected = shown(text.substring(with: part), of: kind)
        guard !selected.isEmpty else { continue }
        plain.append(selected)
        guard case .assistant(let source, let doc, _) = kind else {
          markdown.append(selected)
          continue
        }
        if part != full {
          markdown.append(partial(doc, part, from: full.location, in: text))
        } else {
          markdown.append(source.isEmpty ? DocMarkdown.writer.markdown(doc) : source)
        }
      }
    }
    return (markdown.joined(separator: "\n\n"), plain.joined(separator: "\n"))
  }

  /// A prompt's or a thought's text as the reader sees it: without the tiles'
  /// and the fold header's attachment characters (T37.23.4) or the line breaks
  /// around them; a reply's without its rules' (T37.23.8). Any other block's
  /// text as it is.
  static func shown(_ text: String, of kind: BlockKind?) -> String {
    switch kind {
    case .user?, .thinking?:
      text.replacing("\u{FFFC}", with: "").trimmingCharacters(in: .newlines)
    case .assistant?: text.replacing(RuleAttachment.mark, with: "")
    default: text
    }
  }

  /// A card's summary line; `nil` for a block that is not a card.
  static func card(_ kind: BlockKind) -> String? {
    switch kind {
    case .tool(_, let summary, _, _, _, _, _, _, _, _), .toolGroup(let summary, _, _),
      .approval(_, _, let summary, _, _, _, _, _, _):
      return summary
    case .question(_, let question, _, _): return question
    case .task(_, let label, _, _, _, _, _, _): return label
    default: return nil
    }
  }

  /// Part of a reply: each doc block laid out as `TranscriptText.run` laid it
  /// out from `start`; one wholly selected gives its Markdown, a code block
  /// cut short stays fenced, and other text is the selected text.
  static func partial(
    _ doc: StyledDoc, _ part: NSRange, from start: Int, in text: NSString
  ) -> String {
    var out: [String] = []
    var location = start
    for block in doc.blocks {
      let run = TranscriptText.run(block)
      guard !run.text.isEmpty else { continue }
      let range = NSRange(location: location, length: (run.text as NSString).length)
      location = NSMaxRange(range) + (TranscriptText.separator as NSString).length
      let cut = NSIntersectionRange(range, part)
      guard cut.length > 0 else { continue }
      let selected = text.substring(with: cut)
      if cut == range, let markdown = DocMarkdown.writer.markdown(block) {
        out.append(markdown)
      } else if case .code(let lang, _) = block {
        // A cut through code stays fenced: the writer fences the selected lines as one block.
        out.append(
          DocMarkdown.writer.markdown(.code(lang: lang, lines: [[Span(text: selected)]]))
            ?? selected)
      } else {
        out.append(selected)
      }
    }
    return out.joined(separator: "\n\n")
  }
}
