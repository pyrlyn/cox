// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The app's host (DT§4.4, T37.30.2): `secret` reads `KeychainSecretStore`,
// `notify` posts `NotificationActions`' content with its Allow, Deny or
// Answer actions through `UNUserNotificationCenter`, `open` goes through
// `NSWorkspace`, and a remote host's link only after an alert (T52.20).
// Separate from the Keychain store because this is the AppKit side; the
// store stays usable without it. Only `secret` and the URL
// check are tested: the notification centre needs an app bundle, and a test
// must never post a notification or open a URL. T51.9: with a
// `BrowserController`, the browser methods drive its page.

import AppKit
import CoxClient
import Foundation
import UserNotifications

public struct MacHost: PlatformHost {
  private let secrets: any SecretStore
  private let browser: BrowserController?

  public init(
    secrets: any SecretStore = KeychainSecretStore(), browser: BrowserController? = nil
  ) {
    (self.secrets, self.browser) = (secrets, browser)
  }

  /// A Keychain error reads as no key: Rust then reports the key missing and
  /// names the setting, which is the remedy the person can act on.
  public func secret(for section: String) -> String? {
    try? secrets.secret(for: section)
  }

  public func notify(_ note: HostNote) {
    Task {
      let center = UNUserNotificationCenter.current()
      let allowed = try? await center.requestAuthorization(options: [.alert, .badge, .sound])
      guard allowed == true else { return }
      center.setNotificationCategories(NotificationActions.categories)
      let request = UNNotificationRequest(
        identifier: UUID().uuidString, content: NotificationActions.content(for: note),
        trigger: nil)
      try? await center.add(request)
      try? await center.setBadgeCount(note.badge)
    }
  }

  public func badge(_ count: Int) {
    Task { try? await UNUserNotificationCenter.current().setBadgeCount(count) }
  }

  public func open(_ url: String) {
    guard let url = Self.openable(url) else { return }
    Task { @MainActor in _ = NSWorkspace.shared.open(url) }
  }

  /// A remote host's link (T52.20): the alert names the host and the whole URL, and only Open
  /// opens it.
  public func confirmOpen(_ url: String, from origin: String) {
    guard let url = Self.openable(url) else { return }
    Task { @MainActor in
      let alert = NSAlert()
      alert.messageText = "Open a link from \(origin)?"
      alert.informativeText = url.absoluteString
      alert.addButton(withTitle: "Open")
      alert.addButton(withTitle: "Cancel")
      if alert.runModal() == .alertFirstButtonReturn { _ = NSWorkspace.shared.open(url) }
    }
  }

  public var hasBrowser: Bool { browser != nil }

  public func browserLoad(_ url: String) async throws(BrowserFailure) {
    guard let browser else { throw .noPage }
    try await browser.load(url)
  }

  public func browserText() async throws(BrowserFailure) -> PageText {
    guard let browser else { throw .noPage }
    return try await browser.text()
  }

  public func browserSnapshot() async throws(BrowserFailure) -> [UInt8] {
    guard let browser else { throw .noPage }
    return try await browser.snapshot()
  }

  /// Web links only: the URL comes from an MCP server or the model, and a
  /// `file:` or another app's scheme would launch something, not show a page.
  static func openable(_ text: String) -> URL? {
    guard let url = URL(string: text), let scheme = url.scheme?.lowercased(),
      scheme == "https" || scheme == "http", url.host() != nil
    else { return nil }
    return url
  }
}
