// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `IconTile` (DS§6.2 row `IconTile`, the mockup's `.tool .ic.c-*`): a tool's SF Symbol on a
// lifted, gradient tile coloured by the kind of work. Separate so a tool reads the same in the
// transcript, the inspector and the approval card.

import SwiftUI

/// A `Size.iconTile` tile at e1; the symbol is decorative, the row beside it names the tool.
public struct IconTile: View {
  public enum Kind: CaseIterable, Sendable {
    case neutral, edit, shell, search, write
  }

  /// Top and bottom of the tile's gradient.
  let face: [Color]
  let glyph: Color
  /// An SF Symbol from the DS§3.7 map.
  let symbol: String

  init(_ kind: Kind, symbol: String) {
    self.init(face: kind.face, glyph: kind.glyph, symbol: symbol)
  }

  /// A tile in colours its caller owns, as a Settings page's (A96).
  init(face: [Color], glyph: Color, symbol: String) {
    self.face = face
    self.glyph = glyph
    self.symbol = symbol
  }

  public var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.s, style: .continuous)
    Image(systemName: symbol)
      // The mockup's 13 pt glyph is `font.body`'s size, so it scales with the text size.
      .symbolStyle(.body)
      .foregroundStyle(glyph)
      .frame(width: Size.iconTile, height: Size.iconTile)
      .background {
        shape.fill(LinearGradient(colors: face, startPoint: .top, endPoint: .bottom))
      }
      .elevation(.e1, cornerRadius: Radius.s)
      .accessibilityHidden(true)
  }
}

extension IconTile.Kind {
  var face: [Color] {
    switch self {
    case .neutral: [Color(.tileNeutralTop), Color(.tileNeutralBottom)]
    case .edit: [Color(.tileEditTop), Color(.tileEditBottom)]
    case .shell: [Color(.tileShellTop), Color(.tileShellBottom)]
    case .search: [Color(.tileSearchTop), Color(.tileSearchBottom)]
    case .write: [Color(.tileWriteTop), Color(.tileWriteBottom)]
    }
  }

  var glyph: Color {
    switch self {
    case .neutral: Color(.tileNeutralGlyph)
    case .edit: Color(.tileEditGlyph)
    case .shell: Color(.tileShellGlyph)
    case .search: Color(.tileSearchGlyph)
    case .write: Color(.tileWriteGlyph)
    }
  }
}

#Preview("neutral") {
  PreviewMatrix { IconTile(.neutral, symbol: PreviewState.symbol(.neutral)) }
}
#Preview("edit") { PreviewMatrix { IconTile(.edit, symbol: PreviewState.symbol(.edit)) } }
#Preview("shell") { PreviewMatrix { IconTile(.shell, symbol: PreviewState.symbol(.shell)) } }
#Preview("search") {
  PreviewMatrix { IconTile(.search, symbol: PreviewState.symbol(.search)) }
}
#Preview("write") { PreviewMatrix { IconTile(.write, symbol: PreviewState.symbol(.write)) } }
