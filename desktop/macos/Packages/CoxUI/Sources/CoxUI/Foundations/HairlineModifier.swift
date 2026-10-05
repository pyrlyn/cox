// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `.hairline(_:)` (DS§6.1): the 0.5 pt `separator` line, on chosen edges or around a shape.
// Separate so every separator has one width and one colour.

import SwiftUI

extension View {
  /// Draws a hairline along `edges`, inside the view's bounds.
  func hairline(_ edges: Edge.Set = .all) -> some View {
    overlay { HairlineEdges(edges: edges).allowsHitTesting(false) }
  }

  /// Draws a hairline border just inside `shape`; capsules pass `surface.capsuleBorder`.
  func hairline(in shape: some InsettableShape, color: Color = Color(.separator)) -> some View {
    overlay {
      shape.strokeBorder(color, lineWidth: Size.hairline).allowsHitTesting(false)
    }
  }
}

private struct HairlineEdges: View {
  let edges: Edge.Set

  var body: some View {
    ZStack {
      ForEach(Edge.allCases.filter { edges.contains(Edge.Set($0)) }, id: \.self) {
        EdgeLine(edge: $0)
      }
    }
  }
}

private struct EdgeLine: View {
  let edge: Edge

  var body: some View {
    let across = edge == .top || edge == .bottom
    Color(.separator)
      .frame(width: across ? nil : Size.hairline, height: across ? Size.hairline : nil)
      .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: alignment)
  }

  private var alignment: Alignment {
    switch edge {
    case .top: .top
    case .bottom: .bottom
    case .leading: .leading
    case .trailing: .trailing
    }
  }
}
