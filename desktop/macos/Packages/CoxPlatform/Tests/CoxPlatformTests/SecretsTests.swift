// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// KeychainSecretStore over an in-memory Keychain (A49): the items it asks
// for are the CLI's `cox/<section>` generic passwords, a second store
// replaces rather than duplicates, and a failing status surfaces.

import Foundation
import Security
import Synchronization
import Testing

@testable import CoxPlatform

/// Generic-password items keyed by `service/account`, answering the way
/// `SecItem*` does for the calls the store makes.
final class MemoryKeychain: Sendable {
  let items = Mutex<[String: Data]>([:])
  /// Overrides every call's status when set.
  let failure = Mutex<OSStatus?>(nil)

  static func name(_ query: [String: Any]) -> String? {
    guard (query[kSecClass as String] as? String) == (kSecClassGenericPassword as String),
      let service = query[kSecAttrService as String] as? String,
      let account = query[kSecAttrAccount as String] as? String
    else { return nil }
    return "\(service)/\(account)"
  }

  var calls: KeychainCalls {
    KeychainCalls(
      copy: { query in
        if let status = self.failure.withLock({ $0 }) { return (status, nil) }
        guard let name = Self.name(query) else { return (errSecParam, nil) }
        let data = self.items.withLock { $0[name] }
        return (data == nil ? errSecItemNotFound : errSecSuccess, data)
      },
      add: { attributes in
        if let status = self.failure.withLock({ $0 }) { return status }
        guard let name = Self.name(attributes) else { return errSecParam }
        return self.items.withLock { items in
          guard items[name] == nil else { return errSecDuplicateItem }
          items[name] = attributes[kSecValueData as String] as? Data
          return errSecSuccess
        }
      },
      update: { query, changes in
        if let status = self.failure.withLock({ $0 }) { return status }
        guard let name = Self.name(query) else { return errSecParam }
        return self.items.withLock { items in
          guard items[name] != nil else { return errSecItemNotFound }
          items[name] = changes[kSecValueData as String] as? Data
          return errSecSuccess
        }
      },
      delete: { query in
        if let status = self.failure.withLock({ $0 }) { return status }
        guard let name = Self.name(query) else { return errSecParam }
        return self.items.withLock { $0.removeValue(forKey: name) } == nil
          ? errSecItemNotFound : errSecSuccess
      }
    )
  }
}

@Test func aStoredKeyIsTheClisKeyringItem() throws {
  let keychain = MemoryKeychain()
  let store = KeychainSecretStore(calls: keychain.calls)
  try store.store("sk-one", for: "anthropic")
  #expect(keychain.items.withLock { $0 } == ["cox/anthropic": Data("sk-one".utf8)])
  #expect(try store.secret(for: "anthropic") == "sk-one")
}

@Test func storingAgainReplacesTheItem() throws {
  let keychain = MemoryKeychain()
  let store = KeychainSecretStore(calls: keychain.calls)
  try store.store("sk-one", for: "openai")
  try store.store("sk-two", for: "openai")
  #expect(keychain.items.withLock { $0.count } == 1)
  #expect(try store.secret(for: "openai") == "sk-two")
}

@Test func aMissingKeyIsNilAndRemovingItIsNotAnError() throws {
  let store = KeychainSecretStore(calls: MemoryKeychain().calls)
  #expect(try store.secret(for: "deepseek") == nil)
  try store.remove(for: "deepseek")
  try store.store("sk", for: "deepseek")
  try store.remove(for: "deepseek")
  #expect(try store.secret(for: "deepseek") == nil)
}

@Test func aFailingKeychainThrowsItsStatus() {
  let keychain = MemoryKeychain()
  keychain.failure.withLock { $0 = errSecAuthFailed }
  let store = KeychainSecretStore(calls: keychain.calls)
  #expect(throws: KeychainError.status(errSecAuthFailed)) { try store.secret(for: "anthropic") }
  #expect(throws: KeychainError.status(errSecAuthFailed)) { try store.store("k", for: "anthropic") }
  #expect(throws: KeychainError.status(errSecAuthFailed)) { try store.remove(for: "anthropic") }
}

@Test func anItemThatIsNotTextThrows() {
  let keychain = MemoryKeychain()
  keychain.items.withLock { $0["cox/anthropic"] = Data([0xFF, 0xFE]) }
  let store = KeychainSecretStore(calls: keychain.calls)
  #expect(throws: KeychainError.notText) { try store.secret(for: "anthropic") }
}
