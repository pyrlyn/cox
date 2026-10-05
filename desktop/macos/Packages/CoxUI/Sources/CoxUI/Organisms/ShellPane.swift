// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ShellPane` (DS§4, DS§6.4 row `ShellPane`, the mockup's `.window`, `.sidebar`, `.col` and
// `.insp`): the glass layers the window shell is built from — the window over the wallpaper and
// the panes that float inside it with the wallpaper showing between them. Separate so every
// pane takes its shape, lift and surface from one table, and `MainScreen` composes panes
// without styling one (DS§5).

import SwiftUI

/// `content` on a chrome glass layer of `kind`: the material, window opacity and Depth come
/// from `coxAppearance` through `glassPane` and `elevation`, so `[desktop.appearance]` and
/// Reduce Transparency reach every pane alike. The panes tint the window's behind-window blur
/// rather than frost it again, so the wallpaper's colour shows through at the window opacity.
struct ShellPane<Content: View>: View {
  let kind: ShellPaneKind
  let content: Content
  @EffectiveAppearance private var appearance

  init(_ kind: ShellPaneKind, @ViewBuilder content: () -> Content) {
    self.kind = kind
    self.content = content()
  }

  var body: some View {
    let shape = RoundedRectangle(cornerRadius: kind.radius, style: .continuous)
    // A pane fills the room it is given, even while its slot is still empty.
    ZStack(alignment: .top) {
      Color.clear
      content
    }
    .clipShape(shape)
    .glassPane(shape, surface: kind.fill(appearance), role: kind.role(appearance), frosts: false)
    .hairline(in: shape, color: kind.rim(appearance))
    .elevation(kind.elevation, cornerRadius: kind.radius)
  }
}

/// The layers of the window shell.
enum ShellPaneKind: CaseIterable, Sendable {
  /// The window over the wallpaper, the highest lift (e5).
  case window
  /// The session list, lifted as a card (e2) on the sidebar surface.
  case sidebar
  /// The transcript column: flat glass, so the cards and composer inside it lift from it.
  case column
  /// The inspector, lifted as a card (e2).
  case inspector

  /// DS§3.3: the radius grows with the size of the thing.
  var radius: CGFloat {
    switch self {
    case .window: Radius.window
    case .sidebar: Radius.panel
    case .column, .inspector: Radius.pane
    }
  }

  /// DS§3.4: the window over the wallpaper, the side panes as cards; the column stays flat.
  var elevation: ElevationToken {
    switch self {
    case .window: .e5
    case .sidebar, .inspector: .e2
    case .column: .e0
    }
  }

  /// DS§3.5: on glass the panes take `glass.fill` over the window's own `surface.window` tint,
  /// as mockups 28, 31 and 32 draw them; in Solid (and under Reduce Transparency) every layer is
  /// its plain surface.
  func fill(_ appearance: Appearance) -> Color {
    switch (self, appearance.material) {
    case (.sidebar, .solid): Color(.surfaceSidebar)
    case (.window, _), (_, .solid): Color(.surfaceWindow)
    case (.sidebar, _), (.column, _), (.inspector, _): Color(.glassFill)
    }
  }

  /// `glass.fill` carries its own alpha, and its High Contrast value is in the palette, so the
  /// window opacity does not scale it again; every other fill follows the window opacity.
  func role(_ appearance: Appearance) -> SurfaceRole {
    self != .window && appearance.material != .solid ? .tint : .chrome
  }

  /// The rim: `glass.border` round every glass layer, the window's too; a hairline in Solid.
  func rim(_ appearance: Appearance) -> Color {
    appearance.material == .solid ? Color(.separator) : Color(.glassBorder)
  }
}

#Preview("panes") {
  PreviewMatrix {
    HStack(spacing: Size.paneGap) {
      ForEach(ShellPaneKind.allCases, id: \.self) { kind in
        ShellPane(kind) { Color.clear }.frame(width: Size.capsuleHeight * 2)
      }
    }
    .frame(height: Size.toolbarHeight * 2)
  }
}
