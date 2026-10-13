// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The launch-wide services' test values (T63.4.1, DT§4.6): a test that names no override reads
// stand-ins — keys in memory, never the Keychain (A49), and no inbox — and a test that opens a
// session without saying which core is reported, not silently given one.

import CoxClient
import Dependencies
import DependenciesTestSupport
import Testing

@Test(.dependencies) func aTestReadsTheStandIns() {
  @Dependency(\.secretStore) var secrets
  @Dependency(\.inboxClient) var inbox
  #expect(secrets is MemorySecretStore)
  #expect(inbox == nil)
}

@Test(.dependencies) func openingACoreNoTestProvidedIsReportedAndThrows() async {
  @Dependency(\.coreClient) var core
  var thrown: (any Error)?
  await withKnownIssue {
    do {
      _ = try await core.open(OpenSession(cwd: "/", theme: "t"))
    } catch {
      thrown = error
    }
  }
  #expect((thrown as? MissingDependency)?.name == "coreClient")
}

@Test(.dependency(\.secretStore, MemorySecretStore(["anthropic": "k"])))
func aTraitOverridesOneValueForItsTest() throws {
  @Dependency(\.secretStore) var secrets
  #expect(try secrets.secret(for: "anthropic") == "k")
}
