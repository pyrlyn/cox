// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A Spotlight result opens its session (T51.16): the continued activity names the session, and
// it opens in a window of its own, joining the store if a window has it open already. Apart
// from `CoxApp` because it needs the scene's `openWindow`.

import AppKit
import CoreSpotlight
import CoxPlatform
import SwiftUI

extension View {
  func opensSpotlightResults() -> some View { modifier(SpotlightResults()) }
}

private struct SpotlightResults: ViewModifier {
  @Environment(\.openWindow) private var openWindow

  func body(content: Content) -> some View {
    content.onContinueUserActivity(CSSearchableItemActionType) { activity in
      guard let session = SpotlightIndex.session(from: activity) else { return }
      NSApp.activate()
      openWindow(value: PopOut(session: session, asTab: false))
    }
  }
}
