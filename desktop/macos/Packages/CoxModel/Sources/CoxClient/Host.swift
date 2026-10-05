// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// What the Rust core asks macOS to do (DT§4.4's `Host`), in `CoxClient`
// values: the seam between CoxPlatform, which implements it over the
// Keychain and AppKit, and CoxCore, which adapts it to the generated
// `AppHost`. Neither package depends on the other, so the Keychain side
// tests without the XCFramework and CoxCore never links AppKit.

/// Implemented by CoxPlatform's `MacHost`; called from Rust's threads.
public protocol PlatformHost: Sendable {
  /// The key stored for a provider section, `nil` when there is none.
  func secret(for section: String) -> String?
  /// A new inbox item for a notification and the Dock badge.
  func notify(_ note: HostNote)
  /// The Dock badge fell with no new item: an approval or question was
  /// answered, or its session closed.
  func badge(_ count: Int)
  /// An MCP server's login page or a link; the host decides what it opens.
  func open(_ url: String)
  /// T52.20: a web link a session on the ssh host `origin` asked to show. That machine chose it,
  /// so the host opens it only once the person confirms.
  func confirmOpen(_ url: String, from origin: String)
  /// T51.8: whether there is a page the agent may drive (the browser
  /// pane). Without one, sessions get no browser tools.
  var hasBrowser: Bool { get }
  /// Loads `url`, which Rust already checked is `http` or `https`.
  func browserLoad(_ url: String) async throws(BrowserFailure)
  /// The page's title, address and visible text.
  func browserText() async throws(BrowserFailure) -> PageText
  /// The visible page as PNG bytes.
  func browserSnapshot() async throws(BrowserFailure) -> [UInt8]
}

/// A host with no browser pane: every host but the one that has it.
extension PlatformHost {
  public var hasBrowser: Bool { false }
  public func browserLoad(_ url: String) async throws(BrowserFailure) { throw .noPage }
  public func browserText() async throws(BrowserFailure) -> PageText { throw .noPage }
  public func browserSnapshot() async throws(BrowserFailure) -> [UInt8] { throw .noPage }
}

/// A host that cannot ask opens no remote link.
extension PlatformHost {
  public func confirmOpen(_ url: String, from origin: String) {}
}

/// What the browser pane reports of its page (T51.8).
public struct PageText: Sendable, Equatable {
  public var title: String
  public var url: String
  /// The page's visible text (`document.body.innerText`).
  public var text: String

  public init(title: String, url: String, text: String) {
    (self.title, self.url, self.text) = (title, url, text)
  }
}

/// Why the browser pane could not do what the agent asked.
public enum BrowserFailure: Error, Sendable, Equatable {
  /// Nothing is loaded yet.
  case noPage
  /// The page did not load, or could not be read or captured.
  case page(String)
}

/// An inbox item reduced to what a notification shows.
public struct HostNote: Sendable, Equatable {
  public enum Kind: Sendable, Equatable {
    case approval
    case question
    case failed
    case taskDone(succeeded: Bool)
  }

  /// The session the item belongs to; groups its notifications.
  public var session: String
  public var kind: Kind
  /// The tool awaiting approval, the question, the error or the task label.
  public var text: String
  /// Items that block a turn, across all sessions.
  public var badge: Int
  /// The approval or question a notification action answers; `nil` for news.
  public var call: String?

  public init(session: String, kind: Kind, text: String, badge: Int, call: String? = nil) {
    (self.session, self.kind, self.text, self.badge, self.call) = (
      session, kind, text, badge, call
    )
  }
}
