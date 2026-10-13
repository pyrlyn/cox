// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// T33.45.7's mapping check (PL§15.4): a plugin's image and a span's link survive the step from
// CoxClient's `PluginView` to CoxUI's `PluginWidget`, so the view can draw the one and hand the
// other to `open_link`.

import CoxClient
import CoxUI
import Foundation
import Testing

@testable import CoxTranscript

@Test func anImageAndALinkReachTheWidget() {
  let view = PluginView.stack(
    vertical: true,
    children: [
      .image(alt: "graph", png: [0x89, 0x50]),
      .text(lines: [[PluginRun("src/http.rs:42", link: "src/http.rs:42")]]),
    ])
  #expect(
    PluginWidget(view)
      == .stack(
        vertical: true,
        children: [
          .image(alt: "graph", png: Data([0x89, 0x50])),
          .text([[PluginSpan("src/http.rs:42", link: "src/http.rs:42")]]),
        ]))
}
