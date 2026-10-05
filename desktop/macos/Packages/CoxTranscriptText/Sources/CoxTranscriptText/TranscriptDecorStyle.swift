// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// What a prompt's bubble and a thought's fold are drawn with (T37.23.4, T37.23.9): the values
// the app fills from CoxUI's tokens. Apart from `TranscriptDecor.swift`, which sets and draws
// them, so the drawing file holds only the drawing.

import AppKit

extension TranscriptStyle {
  /// A user prompt's face behind its text and its attachments' tiles.
  public struct Bubble: Equatable {
    public var fill: NSColor
    public var radius: CGFloat
    /// From the bubble's edge to its text.
    public var padding: NSSize
    /// Between two attachment tiles, and between the prompt's text and their row.
    public var gap: CGFloat
    /// The face under `fill`; clear draws none.
    public var face: NSColor
    /// The glass sweep over the face, from its top leading corner to its bottom trailing one.
    public var sweep: [SweepStop]
    /// The lift, bottom first: drop shadows behind the bubble, and inset ones, the
    /// highlight along its edge.
    public var shadows: [Shadow]

    public init(
      fill: NSColor, radius: CGFloat, padding: NSSize, gap: CGFloat, face: NSColor = .clear,
      sweep: [SweepStop] = [], shadows: [Shadow] = []
    ) {
      (self.fill, self.radius, self.padding, self.gap) = (fill, radius, padding, gap)
      (self.face, self.sweep, self.shadows) = (face, sweep, shadows)
    }

    /// How far the drop shadows reach past the bubble: a blur's faint tail runs on to about
    /// twice its CSS length, and cut short it shows as a seam.
    var reach: CGFloat {
      shadows.filter { !$0.inset }
        .map { max(abs($0.offset.width), abs($0.offset.height)) + 2 * $0.blur + $0.spread }
        .max() ?? 0
    }

    public static var system: Bubble {
      Bubble(fill: .quaternarySystemFill, radius: 0, padding: .zero, gap: 0)
    }
  }

  /// One stop of a bubble's glass sweep.
  public struct SweepStop: Equatable {
    public var color: NSColor
    public var location: CGFloat

    public init(color: NSColor, location: CGFloat) {
      (self.color, self.location) = (color, location)
    }
  }

  /// One layer of a bubble's lift as CSS draws a box shadow; `offset` is in the text's
  /// coordinates, down positive.
  public struct Shadow: Equatable {
    public var color: NSColor
    public var offset: CGSize
    public var blur: CGFloat
    public var spread: CGFloat
    public var inset: Bool

    public init(color: NSColor, offset: CGSize, blur: CGFloat, spread: CGFloat, inset: Bool) {
      (self.color, self.offset, self.blur) = (color, offset, blur)
      (self.spread, self.inset) = (spread, inset)
    }
  }

  /// A prompt's turn number in the margin left of its bubble (T37.47; Figma frames 01 and 14,
  /// CoxUI's `TurnGutter`): right-aligned in a box `width` wide whose leading edge is `offset`
  /// left of the bubble's, centred on the prompt's first line.
  public struct Gutter: Equatable {
    public var font: NSFont
    public var color: NSColor
    public var width: CGFloat
    public var offset: CGFloat

    public init(font: NSFont, color: NSColor, width: CGFloat, offset: CGFloat) {
      (self.font, self.color, self.width, self.offset) = (font, color, width, offset)
    }
  }

  /// A thought's reasoning and the rule beside it.
  public struct Thought: Equatable {
    public var font: NSFont
    public var color: NSColor
    public var rule: NSColor
    public var ruleWidth: CGFloat
    /// From the rule to the reasoning.
    public var indent: CGFloat

    public init(font: NSFont, color: NSColor, rule: NSColor, ruleWidth: CGFloat, indent: CGFloat) {
      (self.font, self.color, self.rule) = (font, color, rule)
      (self.ruleWidth, self.indent) = (ruleWidth, indent)
    }

    public static var system: Thought {
      Thought(
        font: .preferredFont(forTextStyle: .body), color: .secondaryLabelColor,
        rule: .separatorColor,
        ruleWidth: 1, indent: 0)
    }
  }
}
