// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Provider keys in the macOS Keychain (DT§5.7, research.md 9.5.8): generic
// password items under service `cox`, account = provider section — the item
// the CLI's `keyring::Entry::new("cox", section)` (cox-provider-http) reads
// and writes, so a key stored here serves `cox` in a terminal too. The file
// Keychain, not the data-protection one, for that reason. The four
// `SecItem` calls sit behind `KeychainCalls` so tests run them against a map
// and never touch the real Keychain (A49).

import CoxClient
import Foundation
import Security

public enum KeychainError: Error, Equatable {
  /// A `SecItem` call failed with this `OSStatus`.
  case status(OSStatus)
  /// The stored item is not UTF-8 text.
  case notText
}

/// The Security framework calls `KeychainSecretStore` makes.
public struct KeychainCalls: Sendable {
  public var copy: @Sendable (_ query: [String: Any]) -> (OSStatus, Data?)
  public var add: @Sendable (_ attributes: [String: Any]) -> OSStatus
  public var update: @Sendable (_ query: [String: Any], _ changes: [String: Any]) -> OSStatus
  public var delete: @Sendable (_ query: [String: Any]) -> OSStatus

  public init(
    copy: @escaping @Sendable ([String: Any]) -> (OSStatus, Data?),
    add: @escaping @Sendable ([String: Any]) -> OSStatus,
    update: @escaping @Sendable ([String: Any], [String: Any]) -> OSStatus,
    delete: @escaping @Sendable ([String: Any]) -> OSStatus
  ) {
    (self.copy, self.add, self.update, self.delete) = (copy, add, update, delete)
  }

  /// The real Keychain.
  public static let security = KeychainCalls(
    copy: { query in
      var result: CFTypeRef?
      let status = SecItemCopyMatching(query as CFDictionary, &result)
      return (status, result as? Data)
    },
    add: { SecItemAdd($0 as CFDictionary, nil) },
    update: { SecItemUpdate($0 as CFDictionary, $1 as CFDictionary) },
    delete: { SecItemDelete($0 as CFDictionary) }
  )
}

public struct KeychainSecretStore: SecretStore {
  /// The CLI's keyring service name.
  public static let service = "cox"

  private let calls: KeychainCalls

  public init(calls: KeychainCalls = .security) { self.calls = calls }

  public func secret(for section: String) throws -> String? {
    var query = Self.item(section)
    query[kSecReturnData as String] = true
    query[kSecMatchLimit as String] = kSecMatchLimitOne
    let (status, data) = calls.copy(query)
    switch status {
    case errSecSuccess:
      guard let data, let text = String(data: data, encoding: .utf8) else {
        throw KeychainError.notText
      }
      return text
    case errSecItemNotFound:
      return nil
    default:
      throw KeychainError.status(status)
    }
  }

  /// Updates first, so an existing item keeps its access list and the CLI
  /// keeps reading it; adds only when there is none.
  public func store(_ secret: String, for section: String) throws {
    let data = Data(secret.utf8)
    let updated = calls.update(Self.item(section), [kSecValueData as String: data])
    guard updated == errSecItemNotFound else {
      guard updated == errSecSuccess else { throw KeychainError.status(updated) }
      return
    }
    var attributes = Self.item(section)
    attributes[kSecValueData as String] = data
    let added = calls.add(attributes)
    guard added == errSecSuccess else { throw KeychainError.status(added) }
  }

  public func remove(for section: String) throws {
    let status = calls.delete(Self.item(section))
    guard status == errSecSuccess || status == errSecItemNotFound else {
      throw KeychainError.status(status)
    }
  }

  /// The one item a section names.
  static func item(_ section: String) -> [String: Any] {
    [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: service,
      kSecAttrAccount as String: section,
    ]
  }
}
