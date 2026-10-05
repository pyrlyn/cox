// `AppIcon` (the mockup's `.appicon`, Figma frame 22-empty-session): cox's mark — `cx` in the
// mono app-icon face on the app tile's gradient. Separate so the welcome hero, and later the
// first-run window and notifications, draw the one mark from the `tile.app` tokens.

import SwiftUI

/// A `Size.appIconHero` tile, `Radius.xxl` corners, lifted to e2. Decorative: the title beside
/// it names the app.
public struct AppIcon: View {
  public init() {}

  public var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.xxl, style: .continuous)
    Text(verbatim: "cx")
      .textStyle(.monoAppIcon)
      .foregroundStyle(Color(.tileAppGlyph))
      .frame(width: Size.appIconHero, height: Size.appIconHero)
      .background {
        // The mockup's 145° gradient, top leading to bottom trailing.
        shape.fill(
          LinearGradient(
            colors: [Color(.tileAppTop), Color(.tileAppBottom)], startPoint: .topLeading,
            endPoint: .bottomTrailing))
      }
      .elevation(.e2, cornerRadius: Radius.xxl)
      .accessibilityHidden(true)
  }
}

#Preview("app icon") { PreviewMatrix { AppIcon() } }
