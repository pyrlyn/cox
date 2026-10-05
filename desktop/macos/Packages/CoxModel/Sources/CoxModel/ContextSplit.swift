// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The context window's split (A98) as the views that draw it take it: cox-app's `MeterText`
// context figures, each part's kind checked against the four the bar colours. One mapping for
// the token popover (T37.25.2) and the inspector's Context tab (T37.29.3.1), so the two views
// cannot read the same figures differently; the app copies it into CoxUI's types.

import CoxClient

public struct ContextSplit: Equatable, Sendable {
  /// `StackedBar.Kind`: a part's colour role, `context.<kind>`.
  public enum Kind: String, CaseIterable, Sendable {
    case system, tools, instructions, history
  }

  /// A part of the window: `System`, `7.6k`, and its share of the bar.
  public struct Part: Equatable, Sendable {
    public var kind: Kind
    public var label: String
    public var tokens: String
    public var fraction: Double
  }

  /// `Context · 76.4k`.
  public var context = ""
  /// `7.6% of 1M` and `923.6k` free; empty while the window is unknown.
  public var share = "", free = ""
  /// In the bar's order; empty until the core sent the split.
  public var parts: [Part] = []

  public init() {}

  /// A part of a kind the bar does not know is left out rather than drawn in another's colour.
  public init(_ text: MeterText) {
    (context, share, free) = (text.context, text.contextShare, text.contextFree)
    parts = text.contextParts.compactMap { part in
      Kind(rawValue: part.kind).map {
        Part(kind: $0, label: part.label, tokens: part.tokens, fraction: part.share)
      }
    }
  }
}
