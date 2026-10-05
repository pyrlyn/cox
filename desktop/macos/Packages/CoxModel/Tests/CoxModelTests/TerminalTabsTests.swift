// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A session's terminal tabs (T51.6): `+` opens another shell of the same session, closing the
// session closes every handle, a busy tab is what the window asks about, and the tab's title.

import CoxClient
import Testing

@testable import CoxModel

@MainActor
private func emptyStore() -> SessionStore {
  SessionStore(session: FixtureSession(fixture: Fixture(batches: [], snapshot: [])))
}

@MainActor
@Test func closingTheSessionClosesItsTerminals() throws {
  let store = emptyStore()
  let first = try store.openTerminal()
  let second = try store.openTerminal()
  store.closeTerminals()
  let clients = [first, second].compactMap { $0.client as? FixtureTerminal }
  #expect(clients.count == 2)
  #expect(clients.allSatisfy { $0.isClosed })
  #expect(store.terminals.isEmpty)
  #expect(store.terminalSelection == nil)
}

@MainActor
@Test func plusOpensAnotherTerminalAndShowsIt() throws {
  let store = emptyStore()
  let first = try store.openTerminal()
  let second = try store.openTerminal()
  #expect(store.terminals.map(\.id) == [first.id, second.id])
  #expect(first.id != second.id)
  #expect(store.terminalSelection == second.id)
}

@MainActor
@Test func closingATabShowsItsNeighbourAndNeverReusesItsId() throws {
  let store = emptyStore()
  let first = try store.openTerminal()
  let second = try store.openTerminal()
  store.closeTerminal(second.id)
  #expect((second.client as? FixtureTerminal)?.isClosed == true)
  #expect((first.client as? FixtureTerminal)?.isClosed == false)
  #expect(store.terminalSelection == first.id)
  let third = try store.openTerminal()
  #expect(third.id != second.id)
}

@MainActor
@Test func aForegroundJobMakesTheSessionBusy() throws {
  let store = emptyStore()
  let tab = try store.openTerminal()
  #expect(!store.hasBusyTerminal)
  let client = try #require(tab.client as? FixtureTerminal)
  client.setBusy(true)
  #expect(store.hasBusyTerminal)
  store.closeTerminals()
  #expect(!store.hasBusyTerminal)
}

@Test func theTitleIsTheShellThenTheWorktreesBranch() {
  #expect(
    TerminalTab.title(shell: "/bin/zsh", branch: "wt/retry-jitter") == "zsh — wt/retry-jitter")
  #expect(TerminalTab.title(shell: "/opt/homebrew/bin/fish", branch: nil) == "fish")
}
