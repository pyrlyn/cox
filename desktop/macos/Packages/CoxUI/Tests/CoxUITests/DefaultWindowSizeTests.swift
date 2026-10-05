// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `Size.defaultWindow`'s clamp (T37.44.14): a new session window opens at the mockups' 1440×900
// on a screen with room for it, and never larger than a smaller screen's visible frame.

import CoreGraphics
import Testing

@testable import CoxUI

/// A screen's visible frame, `width` × `height` points.
private func screen(_ width: CGFloat, _ height: CGFloat) -> CGSize {
  CGSize(width: width, height: height)
}

@Test func aNewWindowOpensAtTheMockupsSizeOnALargeScreen() {
  #expect(Size.defaultWindow(fitting: screen(2560, 1415)) == screen(1440, 900))
  #expect(screen(Size.windowDefaultWidth, Size.windowDefaultHeight) == screen(1440, 900))
}

@Test func aNewWindowIsClampedToASmallerScreensVisibleFrame() {
  #expect(Size.defaultWindow(fitting: screen(1280, 775)) == screen(1280, 775))
  #expect(Size.defaultWindow(fitting: screen(1512, 862)) == screen(1440, 862))
}
