// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A reply's `StyledDoc` back as Markdown (T37.42, T58.4.27): a `docTail` carries doc blocks, not
// source, so Copy as Markdown needs a writer for a reply without its source. The core owns that
// writer (`cox_render::doc`, T58.4.26): a heading's level, a list item's marker and a quote's
// depth, which the doc carries apart from the text (A92), become Markdown's own `#`, `-` or
// number and `>`. This file holds only the seam, set once at launch, so the rules are not kept
// twice. Its own file because `CoxModel` and `CoxTranscriptText` meet only in these value types
// and neither owns the other.

/// Writes a reply's doc as Markdown.
public protocol DocWriter: Sendable {
  /// Every block as Markdown, joined by a blank line.
  func markdown(_ doc: StyledDoc) -> String
  /// One block as Markdown; `nil` for a table without rows.
  func markdown(_ block: DocBlock) -> String?
}

/// The stand-in for a run without the core (a fixture, a test): the blocks' text, no marks.
/// A fixture carries the reply's source with its doc, so Copy never reaches this for one.
public struct PlainDocWriter: DocWriter {
  public init() {}

  public func markdown(_ doc: StyledDoc) -> String {
    doc.blocks.compactMap { markdown($0) }.joined(separator: "\n\n")
  }

  public func markdown(_ block: DocBlock) -> String? {
    func joined(_ lines: [[Span]]) -> String {
      lines.map { $0.map(\.text).joined() }.joined(separator: "\n")
    }
    switch block {
    case .text(_, let lines): return joined(lines.map(\.spans))
    case .code(_, let lines): return joined(lines)
    case .table(let rows):
      return rows.isEmpty ? nil : rows.map { $0.joined(separator: " ") }.joined(separator: "\n")
    case .rule: return ""
    }
  }
}

public enum DocMarkdown {
  /// The writer every client shares. Set once on the main actor at launch, before a window
  /// opens; a lock would only guard a value that never changes after that.
  public nonisolated(unsafe) static var writer: any DocWriter = PlainDocWriter()
}
