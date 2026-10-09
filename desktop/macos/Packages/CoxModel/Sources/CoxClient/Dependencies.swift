// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The launch-wide services as swift-dependencies values (DT§4.6): the core sessions open on, the
// store provider keys live in, and the inbox across sessions. Declared beside their protocols
// with test and preview values only, because the live ones depend on how the app was launched
// (a fixture replay, `COX_HOME`, `COX_KEYRING`): the app registers them once with
// `prepareDependencies` (`LaunchCore`), and a test overrides one with `withDependencies`.
// `SessionClient` is not one of them: each `SessionStore` holds the session it was opened on,
// and many sessions are open at once.

import Dependencies

extension DependencyValues {
  /// Where sessions open: the live core, a fixture's replay, or a launch whose core failed.
  public var coreClient: any CoreClient {
    get { self[CoreClientKey.self] }
    set { self[CoreClientKey.self] = newValue }
  }

  /// Where provider keys live; Settings writes through it and the host's `secret` reads it.
  public var secretStore: any SecretStore {
    get { self[SecretStoreKey.self] }
    set { self[SecretStoreKey.self] = newValue }
  }

  /// The inbox across every session, `nil` when the core keeps none (a failed launch).
  public var inboxClient: (any InboxClient)? {
    get { self[InboxClientKey.self] }
    set { self[InboxClientKey.self] = newValue }
  }
}

/// A core that opens nothing: what a launch whose core failed registers, so a window says why.
public struct UnavailableCoreClient: CoreClient {
  public let error: any Error

  public init(_ error: any Error) { self.error = error }

  public func open(_ request: OpenSession) async throws -> any SessionClient { throw error }
}

/// The error a test's core throws when the test did not provide one.
public struct MissingDependency: Error, CustomStringConvertible {
  public let name: String
  public var description: String { "no \(name) provided; override it with withDependencies" }
}

private enum CoreClientKey: TestDependencyKey {
  /// A test that opens a session says which core; one that forgets fails loudly.
  static var testValue: any CoreClient { UnimplementedCoreClient() }
  /// A preview replays an empty recording, as a fixture launch would.
  static var previewValue: any CoreClient {
    FixtureCoreClient(fixture: Fixture(batches: [], snapshot: []))
  }
}

/// In memory in every test and preview, so none touches the real Keychain (A49).
private enum SecretStoreKey: TestDependencyKey {
  static var testValue: any SecretStore { MemorySecretStore() }
  static var previewValue: any SecretStore { MemorySecretStore() }
}

private enum InboxClientKey: TestDependencyKey {
  static var testValue: (any InboxClient)? { nil }
  static var previewValue: (any InboxClient)? { nil }
}

private struct UnimplementedCoreClient: CoreClient {
  func open(_ request: OpenSession) async throws -> any SessionClient {
    reportIssue("@Dependency(\\.coreClient) was opened without an override")
    throw MissingDependency(name: "coreClient")
  }
}
