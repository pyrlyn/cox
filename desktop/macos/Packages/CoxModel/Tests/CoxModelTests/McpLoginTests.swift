// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// MCP login through SettingsStore (T37.30.3) over the fixture client: a row shows the core's
// line and button (T58.4.3; the words are `cox_app::mcp_login`'s tests), Log in hands the login
// page to the host's `open`, and after the scripted callback the server shows logged in with Log
// out offered. No browser opens and no keychain is read.

import CoxClient
import Synchronization
import Testing

@testable import CoxModel

/// Remembers the URLs it was asked to open.
final class OpenRecorder: PlatformHost {
  let opened = Mutex<[String]>([])

  func secret(for section: String) -> String? { nil }
  func notify(_ note: HostNote) {}
  func badge(_ count: Int) {}
  func open(_ url: String) { opened.withLock { $0.append(url) } }
}

@MainActor
@Suite struct McpLoginTests {
  let host = OpenRecorder()

  private func store() -> SettingsStore {
    let view = SettingsView(
      settings: [], userFile: "/u/config.toml",
      mcp: [
        McpServer(
          name: "docs", source: "config", login: .loggedOut, detail: "Not logged in",
          action: .logIn),
        McpServer(
          name: "local", source: ".mcp.json", login: .stdio,
          detail: "Runs locally from .mcp.json; no login", action: nil),
      ])
    return SettingsStore(
      client: FixtureSettingsClient(view: view, host: host), secrets: MemorySecretStore(),
      cwd: "/p")
  }

  @Test func aServerShowsLoggedOutThenLoggedInAfterTheCallback() async {
    let store = store()
    await store.load()
    #expect(
      store.logins == [
        McpLoginRow(server: "docs", detail: "Not logged in", action: .logIn),
        McpLoginRow(server: "local", detail: "Runs locally from .mcp.json; no login", action: nil),
      ])

    await store.setLogin("docs", true)
    #expect(host.opened.withLock { $0 } == [FixtureSettingsClient.loginPage])
    #expect(
      store.logins.first
        == McpLoginRow(server: "docs", detail: "Logged in, expires in 1h", action: .logOut))

    await store.setLogin("docs", false)
    #expect(store.logins.first?.action == .logIn)
    #expect(store.failure == nil)
  }

  /// T37.45.4: the badge and log Rust decided reach the row unchanged.
  @Test func aRowCarriesItsStatusAndLog() async {
    let view = SettingsView(
      settings: [], userFile: "/u/config.toml",
      mcp: [
        McpServer(
          name: "sentry", source: ".mcp.json", login: .stdio, detail: "", action: nil,
          status: .failed,
          log: ["skipped: spawn uvx: not found"])
      ])
    let store = SettingsStore(
      client: FixtureSettingsClient(view: view, host: host), secrets: MemorySecretStore(),
      cwd: "/p")
    await store.load()
    #expect(store.logins.first?.status == .failed)
    #expect(store.logins.first?.log == ["skipped: spawn uvx: not found"])
  }

  @Test func aStdioServerHasNoLoginToRun() async {
    let store = store()
    await store.load()
    await store.setLogin("local", true)
    #expect(store.failure != nil)
    #expect(host.opened.withLock { $0 }.isEmpty)
  }
}
