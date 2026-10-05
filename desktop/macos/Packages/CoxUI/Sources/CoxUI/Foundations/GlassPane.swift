// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `.glassPane(_:)` (DS§3.5, DS§6.1): the one place a pane chooses glass or a solid surface, so
// the material setting, the readable floor and Reduce Transparency apply to every pane alike.

import SwiftUI

extension View {
  /// Backs the view with the pane material in `shape`: `surface` tinted to the appearance's
  /// opacity over glass, or opaque `surface` in Solid. `frosts: false` is for the window's own
  /// panes, which sit on the behind-window blur: what is under them is frosted already, and a
  /// second glass layer would frost it again and read as white, so they only tint.
  func glassPane(
    _ shape: some Shape, surface: Color = Color(.surfaceWindow), role: SurfaceRole = .chrome,
    frosts: Bool = true
  ) -> some View {
    modifier(GlassPane(shape: shape, surface: surface, role: role, frosts: frosts))
  }
}

private struct GlassPane<S: Shape>: ViewModifier {
  let shape: S
  let surface: Color
  let role: SurfaceRole
  let frosts: Bool
  @EffectiveAppearance private var appearance

  func body(content: Content) -> some View {
    // Bottom up: the glass, the tint, the sweep, then the content — the sweep under the content
    // so the pane's text keeps its token colour (T37.22.10). `specular` draws nothing in Solid.
    let tinted = content.specular(appearance.specular, in: shape).background {
      shape.fill(surface.opacity(appearance.backgroundOpacity(role)))
    }
    switch appearance.material {
    case .frosted where frosts: tinted.glassEffect(.regular, in: shape)
    case .glossy where frosts: tinted.glassEffect(.clear, in: shape)
    default: tinted
    }
  }
}
