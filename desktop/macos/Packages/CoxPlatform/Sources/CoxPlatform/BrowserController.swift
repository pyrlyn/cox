// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The page the agent drives (T51.9, DT§3.3): a `WebPage` (macOS 26 WebKit,
// R9.3.11) behind the host's browser methods — load, read the page, capture it
// as PNG. The browser pane (T51.10) shows the same `page`, so the person sees
// what the agent reads. Separate from `MacHost` because the page lives on
// the main actor and the host is called from Rust's threads.
//
// Trust (DT§10): the page gets no bridge back to the app — no script message
// handler and no user script — and a non-persistent data store, so the
// agent's page never sees the person's cookies or storage. Its text is
// untrusted; Rust sanitizes and archives it before the model sees it.

import AppKit
import CoxClient
import Foundation
import WebKit

@MainActor
public final class BrowserController {
  /// The page the agent drives; the browser pane shows it.
  public let page: WebPage
  /// The page's content controller, kept only so a test can prove it holds
  /// no user script.
  let contentController: WKUserContentController

  /// How wide a screenshot is drawn, in points: a laptop window's page.
  static let snapshotWidth: CGFloat = 1280

  /// Reads the title, address and visible text in one call. It runs in the
  /// page's own world, which has nothing of the app's to reach.
  static let readScript = """
    return {
      title: document.title,
      url: location.href,
      text: document.body ? document.body.innerText : ""
    };
    """

  public init() {
    var configuration = WebPage.Configuration()
    configuration.websiteDataStore = .nonPersistent()
    contentController = configuration.userContentController
    page = WebPage(configuration: configuration)
  }

  /// Loads `address` and returns once the navigation finishes. Rust passes
  /// only `http`/`https`; the check stays there, in one place.
  public func load(_ address: String) async throws(BrowserFailure) {
    guard let url = URL(string: address) else { throw .page("not a URL: \(address)") }
    try await finish(page.load(url))
  }

  /// Loads `html` as the page, for a local fixture in tests.
  func load(html: String) async throws(BrowserFailure) {
    try await finish(page.load(html: html))
  }

  /// The page's title, address and visible text.
  public func text() async throws(BrowserFailure) -> PageText {
    guard page.url != nil else { throw .noPage }
    let value: Any?
    do {
      value = try await page.callJavaScript(Self.readScript, contentWorld: .page)
    } catch {
      throw .page("the page could not be read: \(error.localizedDescription)")
    }
    guard let fields = value as? [String: Any] else {
      throw .page("the page could not be read")
    }
    return PageText(
      title: fields["title"] as? String ?? "", url: fields["url"] as? String ?? "",
      text: fields["text"] as? String ?? "")
  }

  /// The page as PNG bytes. WebKit's image export is re-encoded as PNG, so
  /// Rust's image check sees one of the formats it accepts.
  public func snapshot() async throws(BrowserFailure) -> [UInt8] {
    guard page.url != nil else { throw .noPage }
    let data: Data
    do {
      data = try await page.exported(as: .image(snapshotWidth: Self.snapshotWidth))
    } catch {
      throw .page("the page could not be captured: \(error.localizedDescription)")
    }
    guard let image = NSBitmapImageRep(data: data),
      let png = image.representation(using: .png, properties: [:])
    else { throw .page("the page could not be captured") }
    return [UInt8](png)
  }

  /// Waits out one navigation; a failed one is the model's to read.
  private func finish(
    _ events: some AsyncSequence<WebPage.NavigationEvent, any Error>
  ) async throws(BrowserFailure) {
    do {
      for try await event in events where event == .finished { return }
    } catch {
      throw .page("the page did not load: \(error.localizedDescription)")
    }
  }
}
