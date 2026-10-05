// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `.textStyle(_:)`'s line height (T37.44.2, DS§3.2): a single line of text is as tall as its
// token's line height, as the mockups' CSS line box is, not the font's own shorter one — the
// sidebar rows, the composer and every label take their height from it.

import AppKit
import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct TextStyleTests {
  @Test(arguments: [FontToken.body, .footnote, .label, .transcript, .monoCode])
  func aSingleLineIsAsTallAsItsTokensLineHeight(_ token: FontToken) {
    let host = NSHostingView(rootView: Text("Ag").textStyle(token).fixedSize())
    #expect(abs(host.fittingSize.height - token.size * token.lineHeight) <= 1)
  }

  @Test func eachFurtherLineAddsOneLineHeight() {
    let one = NSHostingView(rootView: Text("Ag").textStyle(.body).fixedSize())
    let two = NSHostingView(rootView: Text("Ag\nAg").textStyle(.body).fixedSize())
    let body = FontToken.body
    #expect(abs(two.fittingSize.height - one.fittingSize.height - body.size * body.lineHeight) <= 1)
  }
}
