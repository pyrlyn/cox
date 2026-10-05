// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The browser half of `PlatformHost` (T51.8): a host that says nothing about a
// browser pane, like every fixture host, has none, so Rust gives its sessions
// no browser tools, and asking it for a page fails as "no page".

import CoxClient
import Testing

@Test func aHostWithoutAPaneHasNoBrowser() async {
  let host = NoteHost()
  #expect(!host.hasBrowser)
  await #expect(throws: BrowserFailure.noPage) { try await host.browserText() }
  await #expect(throws: BrowserFailure.noPage) { try await host.browserLoad("https://a.test") }
  await #expect(throws: BrowserFailure.noPage) { try await host.browserSnapshot() }
}
