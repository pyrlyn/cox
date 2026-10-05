// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The browser pane's bar (T51.10, DT§3.3): the page's address and state as the fields CoxUI's
// `BrowserPaneChrome.State` holds. The address loses its scheme and a bare root's slash, as
// mockup 25 shows `localhost:5173/checkout`, and only `https` earns the lock. Here, not in
// CoxUI, because these decide what the bar shows (DS§1). The app copies them field for field
// from WebKit's page, which this package does not link.

import Foundation

public struct BrowserBarState: Equatable, Sendable {
  public var address: String
  public var isLoading: Bool
  public var isSecure: Bool
  public var canGoBack: Bool

  /// `url` is the page's address, `nil` before the first load.
  public init(url: URL?, isLoading: Bool = false, canGoBack: Bool = false) {
    let scheme = url?.scheme?.lowercased()
    isSecure = scheme == "https"
    self.isLoading = isLoading
    self.canGoBack = canGoBack
    guard let url, scheme == "http" || scheme == "https" else {
      address = url?.absoluteString ?? ""
      return
    }
    var shown = url.absoluteString.dropFirst((scheme ?? "").count + "://".count)
    if url.path() == "/" && url.query() == nil && url.fragment() == nil && shown.hasSuffix("/") {
      shown = shown.dropLast()
    }
    address = String(shown)
  }
}
