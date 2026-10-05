// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// MacHost over the in-memory Keychain (A49): `secret` answers the stored
// provider key and `nil` otherwise, and only web links pass the URL check.
// Nothing here posts a notification or opens a URL.

import CoxClient
import Security
import Testing

@testable import CoxPlatform

@Test func secretAnswersTheStoredProviderKeyAndNilOtherwise() throws {
  let keychain = MemoryKeychain()
  try KeychainSecretStore(calls: keychain.calls).store("sk-ant", for: "anthropic")
  let host = MacHost(secrets: KeychainSecretStore(calls: keychain.calls))
  #expect(host.secret(for: "anthropic") == "sk-ant")
  #expect(host.secret(for: "openai") == nil)
}

@Test func aFailingKeychainReadsAsNoKey() {
  let keychain = MemoryKeychain()
  keychain.failure.withLock { $0 = errSecAuthFailed }
  #expect(
    MacHost(secrets: KeychainSecretStore(calls: keychain.calls)).secret(for: "anthropic") == nil)
}

@Test func onlyWebLinksAreOpened() {
  #expect(MacHost.openable("https://auth.example.com/login?x=1")?.host() == "auth.example.com")
  #expect(MacHost.openable("http://127.0.0.1:8123/callback") != nil)
  for rejected in [
    "file:///etc/passwd", "javascript:alert(1)", "x-app://run", "not a url", "https:///",
  ] {
    #expect(MacHost.openable(rejected) == nil, "\(rejected)")
  }
}
