// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `KeyCap` (DS§6.2 row `KeyCap`, the mockup's `.kbd`): a keyboard shortcut hint beside a
// control or in a tooltip. Separate so every shortcut is drawn as the same small lifted key.

import SwiftUI

/// The keys on a capsule-glass face with a hairline rim, lifted to e1 (DS§3.4). On an inverted
/// `text.primary` face, as `StopButton`'s, the key is an outline in the face's `surface.window`
/// label colour instead: a light glass key would vanish there.
struct KeyCap: View {
  /// The shortcut as macOS writes it, `⌘K`.
  let keys: String
  let isInverted: Bool

  init(_ keys: String, inverted: Bool = false) {
    self.keys = keys
    self.isInverted = inverted
  }

  var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.xs, style: .continuous)
    Text(keys)
      .textStyle(.micro)
      .foregroundStyle(isInverted ? Color(.surfaceWindow) : Color(.textTertiary))
      .padding(.horizontal, Space.xs)
      .background { if !isInverted { shape.fill(Color(.surfaceCapsule)) } }
      .hairline(in: shape, color: isInverted ? Color(.surfaceWindow) : Color(.separator))
      .elevation(isInverted ? .e0 : .e1, cornerRadius: Radius.xs)
  }
}

#Preview("glass") { PreviewMatrix { KeyCap(PreviewState.keys) } }
#Preview("inverted") { PreviewMatrix { StopButton {} } }
