// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Provider keys (DT§5.7, R§9.5.8): the `SecretStore` seam the Settings
// store writes keys through and the app's `AppHost.secret` reads them from.
// The Keychain implementation lives in CoxPlatform, which may link the
// Security framework; the in-memory one lives here so every test and preview
// runs without touching the real Keychain (A49).

import Synchronization

/// Secrets by provider section (`anthropic`, `openai`, a `[providers.<name>]`),
/// the account the CLI's keyring entry `cox/<section>` uses.
public protocol SecretStore: Sendable {
  /// `nil` when none is stored.
  func secret(for section: String) throws -> String?
  /// Adds or replaces.
  func store(_ secret: String, for section: String) throws
  /// Removing a missing secret is not an error.
  func remove(for section: String) throws
}

/// The Keychain as a map.
public final class MemorySecretStore: SecretStore {
  private let secrets: Mutex<[String: String]>

  public init(_ secrets: [String: String] = [:]) { self.secrets = Mutex(secrets) }

  public func secret(for section: String) throws -> String? {
    secrets.withLock { $0[section] }
  }

  public func store(_ secret: String, for section: String) throws {
    secrets.withLock { $0[section] = secret }
  }

  public func remove(for section: String) throws {
    _ = secrets.withLock { $0.removeValue(forKey: section) }
  }
}
