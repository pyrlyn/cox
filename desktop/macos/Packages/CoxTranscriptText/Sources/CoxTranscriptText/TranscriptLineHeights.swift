// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The transcript's line heights (A93, DS§3.2): each face's `lineHeight` token as the space
// under its lines, set by the paragraph styles every block's text carries
// (`TranscriptText.respace`, `Paragraphs`, `Decor`). Its own file so those three build their
// line spacing one way, the way CoxUI's `.textStyle` does for SwiftUI text.

import AppKit

extension TranscriptStyle {
  /// Each face's line height as a multiple of its font's size; one below the font's own line,
  /// as `natural`'s zeros are, draws the font's own.
  public struct LineHeights: Equatable, Sendable {
    public var body: CGFloat
    public var code: CGFloat
    public var heading: CGFloat
    public var thought: CGFloat

    public init(body: CGFloat, code: CGFloat, heading: CGFloat, thought: CGFloat) {
      (self.body, self.code, self.heading, self.thought) = (body, code, heading, thought)
    }

    public static let natural = LineHeights(body: 0, code: 0, heading: 0, thought: 0)
  }

  /// A paragraph style whose lines of `font` are `multiple` of its size tall: the space under
  /// each line past the font's own height, as `.textStyle` adds it.
  static func lines(_ font: NSFont, _ multiple: CGFloat) -> NSMutableParagraphStyle {
    let paragraph = NSMutableParagraphStyle()
    let natural = font.ascender - font.descender + font.leading
    paragraph.lineSpacing = max(0, font.pointSize * multiple - natural)
    return paragraph
  }
}
