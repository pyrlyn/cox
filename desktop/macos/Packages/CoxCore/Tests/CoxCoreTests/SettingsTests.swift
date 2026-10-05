// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Settings through the real Rust core (T37.30): a scratch home, an edit that
// lands in its `config.toml` and comes back with the `user` layer, an MCP
// server's login (T37.30.3) and a project value the guard list drops
// (T37.30.4). The host has no secrets and `COX_KEYRING=off`
// keeps Rust's MCP token reads away from the Keychain (A49).

import CoxClient
import CoxFFIBindings
import Foundation
import Testing

@testable import CoxCore

final class SilentHost: AppHost {
  func notify(item: CoxFFIBindings.InboxItem, badge: UInt32) {}
  func badge(badge: UInt32) {}
  func openUrl(url: String) {}
  func confirmOpenUrl(origin: String, url: String) {}
  func secret(section: String) -> String? { nil }
  func hasBrowser() -> Bool { false }
  func browserLoad(url: String) async throws { throw CoxFFIBindings.BrowserFailure.NoPage }
  func browserText() async throws -> CoxFFIBindings.PageText {
    throw CoxFFIBindings.BrowserFailure.NoPage
  }
  func browserSnapshot() async throws -> Data { throw CoxFFIBindings.BrowserFailure.NoPage }
}

/// A scratch home and a client over it. Settings reads each HTTP MCP server's
/// token, so the keyring is switched off first, as cargo does for Rust tests.
func scratch() throws -> (URL, LiveCoreClient) {
  setenv("COX_KEYRING", "off", 1)
  let home = FileManager.default.temporaryDirectory.appending(path: "cox-settings-\(UUID())")
  return (home, try LiveCoreClient(home: home.path(), host: SilentHost()))
}

@Test func anEditRoundTripsThroughRustIntoTheUserLayer() async throws {
  let (home, client) = try scratch()
  defer { try? FileManager.default.removeItem(at: home) }
  let key = "desktop.appearance.material"

  let before = try await client.settings(cwd: home.path())
  #expect(before.settings.first { $0.key == key }?.layer == .default)

  let after = try await client.setSetting(cwd: home.path(), key: key, json: "\"solid\"")
  let row = try #require(after.settings.first { $0.key == key })
  #expect(row.layer == .user)
  #expect(row.value == "\"solid\"")
  #expect(row.kind == .choice(options: ["frosted", "glossy", "solid"]))
  #expect(after.userFile == home.appending(path: "config.toml").path())
}

@Test func aValueTheLoaderRejectsIsASettingsError() async throws {
  let (home, client) = try scratch()
  defer { try? FileManager.default.removeItem(at: home) }
  await #expect {
    try await client.setSetting(cwd: home.path(), key: "desktop.appearance.opacity", json: "1.5")
  } throws: { error in
    if case AppError.Settings = error { true } else { false }
  }
}

@Test func aStdioServerComesThroughWithNoLoginToRun() async throws {
  let (home, client) = try scratch()
  defer { try? FileManager.default.removeItem(at: home) }
  try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
  try Data("[mcp.servers.cox-local]\ncommand = \"echo\"\n".utf8)
    .write(to: home.appending(path: "config.toml"))

  let view = try await client.settings(cwd: home.path())
  let server = try #require(view.mcp.first { $0.name == "cox-local" })
  #expect(server.login == .stdio)
  // The core's words arrive converted (T58.4.3).
  #expect(server.detail == "Runs locally from \(server.source); no login")
  #expect(server.action == nil)
  await #expect {
    try await client.mcpLogin(cwd: home.path(), server: "cox-local", login: true)
  } throws: { error in
    if case AppError.Settings = error { true } else { false }
  }
}

@Test func aProjectBudgetRaiseComesThroughAsDropped() async throws {
  let (home, client) = try scratch()
  defer { try? FileManager.default.removeItem(at: home) }
  let project = home.appending(path: "project")
  for dir in [".git", ".cox"] {
    try FileManager.default.createDirectory(
      at: project.appending(path: dir), withIntermediateDirectories: true)
  }
  try Data("[budget]\nsession_usd = 999.0\n".utf8)
    .write(to: project.appending(path: ".cox/config.toml"))

  let view = try await client.settings(cwd: project.path())
  let dropped = try #require(view.dropped.first { $0.key == "budget.session_usd" })
  #expect(dropped.value == "999")
  #expect(dropped.reason == "A project may not raise a budget above your own")
}

@Test func aTypedInputAndAProviderKeyAreCheckedInRust() async throws {
  let (home, client) = try scratch()
  defer { try? FileManager.default.removeItem(at: home) }
  let after = try await client.setSettingInput(
    cwd: home.path(), key: "core.max_turns", input: .text(" 12 "))
  let row = try #require(after.settings.first { $0.key == "core.max_turns" })
  #expect(row.value == "12" && row.layer == .user)
  #expect(row.group == .general && row.title == "Max turns" && row.control == .field("12"))
  #expect(after.providers.contains("anthropic"))
  #expect(
    try client.checkKey(providers: after.providers, provider: "anthropic", secret: " k\n") == "k")
  #expect(throws: CoxClient.KeyError.empty) {
    try client.checkKey(providers: after.providers, provider: "anthropic", secret: " ")
  }
  #expect(throws: CoxClient.KeyError.unknownProvider("nope")) {
    try client.checkKey(providers: after.providers, provider: "nope", secret: "k")
  }
}
