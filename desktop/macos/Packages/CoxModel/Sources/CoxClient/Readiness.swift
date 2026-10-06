// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Whether a turn can start (DT§5.3, T60.2, A138), as cox-ffi's `Readiness`. The rule and its
// wording are the core's; this carries the answer to the composer, which only renders it.

/// `cox_app::Readiness` with the text the core words for it.
public struct Readiness: Equatable, Sendable {
  public enum Reason: Equatable, Sendable {
    case ready
    /// `tiers.code.provider` is empty or names no provider section.
    case noProvider
    /// A keyed provider whose key was not found.
    case noKey(provider: String)
    /// A provider on this machine whose server does not accept connections.
    case unreachable(provider: String)
  }

  public var reason: Reason
  /// What to show where sending is disabled (`cox_app::Readiness::message`); `nil` when ready.
  public var message: String?

  public init(_ reason: Reason, message: String? = nil) {
    (self.reason, self.message) = (reason, message)
  }

  public var isReady: Bool { reason == .ready }

  public static let ready = Readiness(.ready)
  public static let noProvider = Readiness(
    .noProvider, message: "No provider is set for the code tier.")

  public static func noKey(provider: String) -> Readiness {
    Readiness(.noKey(provider: provider), message: "No API key for \(provider).")
  }

  public static func unreachable(provider: String) -> Readiness {
    Readiness(.unreachable(provider: provider), message: "\(provider) is not running.")
  }
}

extension Readiness {
  /// Why a turn waits, `nil` when it need not: the core's message, with a stand-in for a client
  /// that decoded none.
  public var blockedReason: String? {
    isReady ? nil : message ?? "No provider can answer."
  }

  /// The notice's action under the composer: a missing key is added in Settings › Providers; a
  /// session with no provider or a local server that is not running is fixed in Settings too,
  /// without a key to add. `nil` when ready (DT§5.3, T60.5).
  public var actionTitle: String? {
    switch reason {
    case .ready: nil
    case .noKey: "Add key"
    case .noProvider, .unreachable: "Open Settings"
    }
  }

  /// Why Best of cannot add a candidate on `provider`: it is not among the `usable` providers the
  /// core probed (`usableProviders`). `nil` while the probe has not answered, or when the
  /// provider is usable or unnamed — a fixture's, which omits it.
  public static func unavailableReason(provider: String, usable: [String]?) -> String? {
    guard let usable, !provider.isEmpty, !usable.contains(provider) else { return nil }
    return "No key for \(provider), or it is not running."
  }
}
