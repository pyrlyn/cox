// `AppIcon` (the mockup's `.appicon`, Figma frame 22-empty-session): cox's mark — the brand's
// terminal-pane tile from brand/logo/cox-mark.svg, kept as a vector in Brand.xcassets by
// `just brand-icons`. Separate so the welcome hero, and later the first-run window and
// notifications, draw the one mark the app icon and the site use.

import SwiftUI

/// A `Size.appIconHero` mark lifted to e2. Decorative: the title beside it names the app.
public struct AppIcon: View {
  public init() {}

  public var body: some View {
    Image(.coxMark)
      .resizable()
      .interpolation(.high)
      .frame(width: Size.appIconHero, height: Size.appIconHero)
      // The mark's own tile corner (rx 12 of 64), so the shadow follows the artwork.
      .elevation(.e2, cornerRadius: Size.appIconHero * 12 / 64)
      .accessibilityHidden(true)
  }
}

#Preview("app icon") { PreviewMatrix { AppIcon() } }
