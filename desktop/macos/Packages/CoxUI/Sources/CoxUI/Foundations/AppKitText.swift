// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// AppKit text from the tokens (DS§3.1, DS§3.2): the fonts and colours a TextKit view outside
// CoxUI draws with — the transcript's one `NSTextView` (T37.23) — resolved from the same tokens
// and colour assets as `.textStyle` and `Color(.<role>)`. In Foundations because only Tokens and
// Foundations may spell a design value (DS§9), and public because the assets are internal.

import AppKit
import SwiftUI

/// The text colours AppKit text may take: the readable ones only (DS§8). Each follows light,
/// dark and Increase Contrast through the asset.
public enum TextColour: CaseIterable, Sendable {
  case primary, secondary, tertiary, accent

  public var nsColor: NSColor {
    switch self {
    case .primary: NSColor(resource: .textPrimary)
    case .secondary: NSColor(resource: .textSecondary)
    case .tertiary: NSColor(resource: .textTertiary)
    case .accent: NSColor(resource: .accent)
    }
  }
}

/// The colours AppKit drawing behind text may take (T37.23.4): the user bubble's tint,
/// readable face and sweep (T37.23.9), the hairline beside a thought and a quote's bar (A97),
/// from the same assets as `Color(.fillPrimary)`, `.glassPane`'s surface, `.specular`,
/// `.hairline` and `quote.bar`.
public enum SurfaceColour: CaseIterable, Sendable {
  case fillPrimary, separator, window, specular, quoteBar

  public var nsColor: NSColor {
    switch self {
    case .fillPrimary: NSColor(resource: .fillPrimary)
    case .separator: NSColor(resource: .separator)
    case .window: NSColor(resource: .surfaceWindow)
    case .specular: NSColor(resource: .glassSpecular)
    case .quoteBar: NSColor(resource: .quoteBar)
    }
  }
}

/// The terminal pane's colours (T51.6): `surface.terminal`, `text.terminal` and
/// `text.terminalOk`, for SwiftTerm's view, which AppKit draws.
public enum TerminalColour: CaseIterable, Sendable {
  case surface, text, success

  public var nsColor: NSColor {
    switch self {
    case .surface: NSColor(resource: .surfaceTerminal)
    case .text: NSColor(resource: .textTerminal)
    case .success: NSColor(resource: .textTerminalOk)
    }
  }
}

extension Appearance {
  /// A readable surface's opacity, as `.glassPane(role: .readable)` sets it (DS§3.5), for a
  /// face AppKit draws (T37.23.9).
  public var readableOpacity: Double { backgroundOpacity(.readable) }

  /// The glass sweep AppKit draws over a readable face, as `.specular` draws it: white stops
  /// along the diagonal from the top leading corner; none in Solid (DS§3.5).
  public var sweepStops: [(opacity: Double, location: Double)] {
    material == .solid || specular <= 0 ? [] : Self.sweep(specular)
  }
}

extension FontToken {
  /// The token's font for AppKit text at `scale`, the appearance's text size (DS§3.2).
  public func nsFont(scale: Double = 1) -> NSFont {
    let size = size * scale
    let weight: NSFont.Weight =
      switch self.weight {
      case .medium: .medium
      case .semibold: .semibold
      case .bold: .bold
      default: .regular
      }
    return design == .monospaced
      ? .monospacedSystemFont(ofSize: size, weight: weight)
      : .systemFont(ofSize: size, weight: weight)
  }
}
