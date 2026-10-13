// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PluginSymbol` (PL§15.4, T33.45.7): the SF Symbol a plugin names for an inspector tab, a toolbar
// item or a palette action, checked against the system's symbol set. Separate so every slot falls
// back the same way: cox-app only checks the name's spelling, so a name this macOS does not ship
// would otherwise draw an empty well.

import AppKit

public enum PluginSymbol {
  /// The glyph drawn for a missing name or one the system does not know.
  public static let fallback = "puzzlepiece.extension"

  public static func name(_ symbol: String?) -> String {
    guard let symbol, NSImage(systemSymbolName: symbol, accessibilityDescription: nil) != nil
    else { return fallback }
    return symbol
  }
}
