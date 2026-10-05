// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `CodeRun` (DS§6.3 rows `DiffLineView` and `CodeBlockView`, the mockup's `.kw`, `.str`, `.num`,
// `.fn`, `.com`, `.ty`): a stretch of code text in one syntax role, as the core highlighted it.
// Separate so a diff line and a code block colour the same roles the same way; the app maps
// Rust `StyledDoc` spans to these runs and a view never highlights on its own. A run the core
// coloured from the session theme (A95) carries that theme's light and dark colour instead.

import SwiftUI

/// Text and the DS§3.1 `syntax` role that colours it; `plain` takes the surrounding colour.
public struct CodeRun: Equatable, Sendable {
  public enum Role: CaseIterable, Sendable {
    case plain, keyword, string, number, function, comment, type
  }

  public var text: String
  public var role: Role
  /// A word the core's word diff found changed in a replaced line pair (T37.23.11).
  public var isChanged: Bool
  /// The session theme's colour for the run, which wins over `role`; `nil` when the core left
  /// the run uncoloured.
  public var theme: Theme?

  /// A theme colour in the theme's light and dark variant, each `0xRRGGBB`.
  public struct Theme: Equatable, Sendable {
    public var light: UInt32
    public var dark: UInt32

    public init(light: UInt32, dark: UInt32) {
      (self.light, self.dark) = (light, dark)
    }
  }

  public init(_ text: String, _ role: Role = .plain, isChanged: Bool = false, theme: Theme? = nil) {
    self.text = text
    self.role = role
    self.isChanged = isChanged
    self.theme = theme
  }

  /// Lines of runs as one attributed string, so one `Text` draws a whole block; a changed run
  /// sits on `mark`.
  static func attributed(_ lines: [[CodeRun]], mark: Color? = nil) -> AttributedString {
    var joined = AttributedString()
    for (index, line) in lines.enumerated() {
      if index > 0 { joined.append(AttributedString("\n")) }
      for run in line { joined.append(run.attributed(mark: mark)) }
    }
    return joined
  }

  private func attributed(mark: Color?) -> AttributedString {
    var text = AttributedString(self.text)
    text.foregroundColor = theme?.colour ?? role.colour
    if isChanged { text.backgroundColor = mark }
    return text
  }
}

extension CodeRun.Role {
  /// `nil` for plain text, which keeps the foreground the view sets.
  var colour: Color? {
    switch self {
    case .plain: nil
    case .keyword: Color(.syntaxKeyword)
    case .string: Color(.syntaxString)
    case .number: Color(.syntaxNumber)
    case .function: Color(.syntaxFunction)
    case .comment: Color(.syntaxComment)
    case .type: Color(.syntaxType)
    }
  }
}

extension CodeRun.Theme {
  /// One dynamic colour that AppKit resolves against the drawing view's `effectiveAppearance`, so
  /// a window switching between light and dark redraws the run in the other variant with no state
  /// of its own.
  var colour: Color {
    let (light, dark) = (light, dark)
    return Color(
      nsColor: NSColor(name: nil) { appearance in
        let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        let rgb = isDark ? dark : light
        let channel = { (shift: UInt32) in CGFloat(rgb >> shift & 0xFF) / 255 }
        // The session theme's colour, not a DS§3 token: A95 lets the theme colour syntax runs.
        // swiftlint:disable:next no_literal_colour
        return NSColor(srgbRed: channel(16), green: channel(8), blue: channel(0), alpha: 1)
      })
  }
}
