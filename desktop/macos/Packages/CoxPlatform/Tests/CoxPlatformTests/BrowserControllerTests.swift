// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// BrowserController over a local HTML fixture (T51.9): it loads the page,
// reads its title, address and text, and captures it as PNG. It registers no
// script message handler, so the page cannot reach the app. With no page it
// says so. MacHost offers the browser only when it was given one. Nothing
// here goes to the network.

import CoxClient
import Foundation
import Testing
import WebKit

@testable import CoxPlatform

private let fixture = """
  <!doctype html>
  <html><head><title>Fixture page</title></head>
  <body><h1>Hello from cox</h1><p>Second line.</p></body></html>
  """

@MainActor
@Suite struct BrowserControllerTests {
  @Test func aLocalPageIsLoadedReadAndCaptured() async throws {
    let browser = BrowserController()
    try await browser.load(html: fixture)
    let page = try await browser.text()
    #expect(page.title == "Fixture page")
    #expect(page.url == "about:blank")
    #expect(page.text.contains("Hello from cox"))
    #expect(page.text.contains("Second line."))
    let png = try await browser.snapshot()
    #expect(png.starts(with: [0x89, 0x50, 0x4E, 0x47]))
  }

  @Test func theControllerRegistersNoMessageHandler() async throws {
    let browser = BrowserController()
    #expect(browser.contentController.userScripts.isEmpty)
    try await browser.load(html: fixture)
    // WebKit defines `window.webkit` in a world only when a message handler
    // is registered there.
    let bridge = try await browser.page.callJavaScript(
      "return typeof window.webkit", contentWorld: .page)
    #expect(bridge as? String == "undefined")
  }

  @Test func withNoPageTheControllerSaysSo() async {
    let browser = BrowserController()
    await #expect(throws: BrowserFailure.noPage) { try await browser.text() }
    await #expect(throws: BrowserFailure.noPage) { try await browser.snapshot() }
  }

  @Test func macHostOffersTheBrowserOnlyWhenGivenOne() async throws {
    let keychain = MemoryKeychain()
    let secrets = KeychainSecretStore(calls: keychain.calls)
    #expect(!MacHost(secrets: secrets).hasBrowser)
    let browser = BrowserController()
    let host = MacHost(secrets: secrets, browser: browser)
    #expect(host.hasBrowser)
    try await browser.load(html: fixture)
    #expect(try await host.browserText().title == "Fixture page")
  }
}
