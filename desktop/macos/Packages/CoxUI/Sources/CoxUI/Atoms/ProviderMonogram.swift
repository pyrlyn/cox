// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ProviderMonogram` (DS§6.2 row `ProviderMonogram`, DS§3.7 "Provider monograms"; T60.7): the
// mark that stands for a provider in the composer's model chip. A vendor's logo is a trademark,
// so cox draws a monogram instead — the display name's initials on an `IconTile` face — and
// draws it at run time from the provider's name and slug alone, so no image asset exists to
// copy by hand (A136). Separate so the chip and any later provider list show one mark.

import SwiftUI

/// A provider as the composer's chip names it: the `[providers.<slug>]` section, which fixes the
/// monogram's colour, and the name a person calls it (`cox_app::models::provider_name`), which
/// gives its letters.
public struct ProviderMark: Equatable, Sendable {
  public var slug: String
  public var name: String

  public init(slug: String, name: String) {
    (self.slug, self.name) = (slug, name)
  }
}

/// The initials in `font.micro` on a rounded `Size.providerMonogram` square, face and glyph from
/// the `tile.*` tokens picked by the slug, so one provider always has one colour.
public struct ProviderMonogram: View {
  let provider: ProviderMark

  public init(_ provider: ProviderMark) { self.provider = provider }

  public var body: some View {
    let kind = Self.kind(of: provider.slug)
    Text(Self.letters(of: provider.name))
      .textStyle(.micro)
      .lineLimit(1)
      .minimumScaleFactor(0.7)
      .foregroundStyle(kind.glyph)
      .frame(width: Size.providerMonogram, height: Size.providerMonogram)
      .background {
        RoundedRectangle(cornerRadius: Radius.xs, style: .continuous)
          .fill(LinearGradient(colors: kind.face, startPoint: .top, endPoint: .bottom))
      }
      .accessibilityHidden(true)
  }

  /// One letter for a one-word name, the first letters of the first two words for more:
  /// `Anthropic` is `A`, `LM Studio` is `LS`. A name with no letter or digit shows `?`.
  static func letters(of name: String) -> String {
    let initials = name.split { !$0.isLetter && !$0.isNumber }.prefix(2).compactMap(\.first)
    return initials.isEmpty ? "?" : String(initials).uppercased()
  }

  /// The tile colour of a slug: FNV-1a over its UTF-8 bytes, modulo the five tile kinds in their
  /// declared order. `hashValue` would change from launch to launch; this is the rule DS§3.7
  /// gives, so another client draws the same colours.
  static func kind(of slug: String) -> IconTile.Kind {
    var hash: UInt32 = 2_166_136_261
    for byte in slug.utf8 { hash = (hash ^ UInt32(byte)) &* 16_777_619 }
    let kinds = IconTile.Kind.allCases
    return kinds[Int(hash) % kinds.count]
  }
}

#Preview("anthropic") { PreviewMatrix { ProviderMonogram(PreviewState.provider) } }
#Preview("two words") { PreviewMatrix { ProviderMonogram(PreviewState.localProvider) } }
