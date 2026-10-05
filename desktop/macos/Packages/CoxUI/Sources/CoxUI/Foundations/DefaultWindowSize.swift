// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A new session window's size (T37.44.14, A126 (3)): the mockups' 1440×900 from
// `size.windowDefault*`, never larger than the screen it opens on. A plain function so a test
// checks the clamp without a screen; the app's scenes hand it the display's visible frame.

import CoreGraphics

extension Size {
  /// The default window, each side cut to `visible`, the screen's frame less the menu bar and
  /// the Dock.
  public static func defaultWindow(fitting visible: CGSize) -> CGSize {
    CGSize(
      width: min(windowDefaultWidth, visible.width),
      height: min(windowDefaultHeight, visible.height))
  }
}
