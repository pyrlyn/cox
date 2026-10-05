// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The session window's browser pane (T51.10, mockup 25, DT§3.3): CoxUI's `BrowserPaneChrome`
// around WebKit's `WebView` of the one page the agent's browser tools drive (CoxPlatform's
// `BrowserController`), so the person sees what the agent reads. Wiring only: a typed address
// passes Rust's http/https rule (`cox_app::browser::web_address`) before it loads, CoxModel
// turns the page's state into the bar's, and the packages draw.

import CoxCore
import CoxModel
import CoxPlatform
import CoxUI
import SwiftUI
import WebKit

struct SessionBrowser: View {
  /// Mockup 25's web column, until the pane learns to resize.
  static let paneWidth: CGFloat = 560

  let controller: BrowserController
  let refused: (String) -> Void

  var body: some View {
    BrowserPaneChrome(state: state, send: handle) {
      WebView(controller.page)
    }
  }

  /// `WebPage` is observable, so the bar follows the page as it loads and navigates.
  private var state: BrowserPaneState {
    let page = controller.page
    let bar = BrowserBarState(
      url: page.url, isLoading: page.isLoading,
      canGoBack: !page.backForwardList.backList.isEmpty)
    return BrowserPaneState(
      address: bar.address, isLoading: bar.isLoading, isSecure: bar.isSecure,
      canGoBack: bar.canGoBack)
  }

  private func handle(_ intent: BrowserPaneIntent) {
    let page = controller.page
    switch intent {
    // The navigations run on their own; the pane only starts them.
    case .back:
      if let item = page.backForwardList.backList.last { _ = page.load(item) }
    case .reload: _ = page.reload()
    case .open(let text):
      guard let address = LiveCoreClient.webAddress(text) else {
        refused("Only http and https addresses open in the browser: \(text)")
        return
      }
      Task {
        do {
          try await controller.load(address)
        } catch {
          refused(String(describing: error))
        }
      }
    }
  }
}
