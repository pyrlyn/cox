// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The secret store's live value (DT§4.6, T63.4.1): here, beside `KeychainSecretStore`, because
// `CoxClient` declares the key with test and preview values only and may not link Security.
// swift-dependencies finds this conformance at run time in the app, which links CoxPlatform;
// a package test that does not link it gets the in-memory test value.

import CoxClient
import Dependencies
import Foundation

// Retroactive on purpose: cox owns `CoxClient` too, which declares no live value by design, so
// the conformance cannot appear there later; `@retroactive` silences the compiler's warning.
// swift-format-ignore: AvoidRetroactiveConformances
extension SecretStoreKey: @retroactive DependencyKey {
  /// One store for the launch: under `COX_KEYRING=off` the host's `secret` must read the key
  /// Settings just stored in memory, so the value is made once, not per read.
  public static let liveValue: any SecretStore = live(ProcessInfo.processInfo.environment)

  /// `COX_KEYRING=off`, as cargo sets it for every development run, keeps provider keys out of
  /// the Keychain (A49, A51): a development launch never raises a Keychain prompt.
  static func live(_ environment: [String: String]) -> any SecretStore {
    environment["COX_KEYRING"] == "off" ? MemorySecretStore() : KeychainSecretStore()
  }
}
