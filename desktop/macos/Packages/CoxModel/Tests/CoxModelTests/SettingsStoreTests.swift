// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// SettingsStore over the fixture client and an in-memory secret store: a
// field the project layer overrides stays read-only with its layer, edits
// reach Rust as JSON, and keys never touch the real Keychain (A49).

import CoxClient
import Testing

@testable import CoxModel

/// The rows `cox_app::settings`' snapshot pins, for a project that overrides the model and a
/// user file that picks the material, with the page, box, label and control Rust gives each.
let fixtureView = SettingsView(
  settings: [
    Setting(
      key: "budget.session_usd", value: "5.0", layer: .default, editable: true,
      kind: .number(min: nil, max: nil), description: "Session spend cap, in USD.",
      group: .budget, title: "Session usd", table: "budget",
      detail: "Session spend cap, in USD.", control: .field("5.0")),
    Setting(
      key: "desktop.appearance.material", value: "\"glossy\"", layer: .user, editable: true,
      kind: .choice(options: ["frosted", "glossy", "solid"]), description: "",
      group: .appearance, title: "Material", table: "desktop.appearance",
      control: .choice("glossy", options: ["frosted", "glossy", "solid"])),
    Setting(
      key: "desktop.appearance.opacity", value: "0.42", layer: .default, editable: true,
      kind: .number(min: 0, max: 1), description: "", group: .appearance, title: "Opacity",
      table: "desktop.appearance", control: .slider(0.42, range: 0...1, text: "0.42")),
    Setting(
      key: "providers.anthropic.base_url", value: "\"https://api.anthropic.com\"",
      layer: .default, editable: true, kind: .text, description: "", group: .models,
      title: "Base url", table: "providers.anthropic", provider: "anthropic",
      control: .field("https://api.anthropic.com")),
    Setting(
      key: "tiers.code.model", value: "\"project-model\"", layer: .project, editable: false,
      kind: .text, description: "The model id sent in the request.", group: .models,
      title: "Model", table: "tiers.code", detail: "Set in /project/.cox/config.toml",
      control: .field("project-model")),
  ],
  userFile: "/home/.cox/config.toml",
  projectFile: "/project/.cox/config.toml",
  providers: ["anthropic"])

typealias Loaded = (store: SettingsStore, client: FixtureSettingsClient)

@MainActor
func loadedStore(secrets: any SecretStore = MemorySecretStore()) async -> Loaded {
  let client = FixtureSettingsClient(view: fixtureView)
  let store = SettingsStore(client: client, secrets: secrets, cwd: "/project")
  await store.load()
  return (store, client)
}

@MainActor
@Test func aSettingTheProjectOverridesIsReadOnlyWithItsLayer() async throws {
  let (store, _) = await loadedStore()
  let models = try #require(store.sections.first { $0.group == .models })
  let model = try #require(models.settings.first { $0.key == "tiers.code.model" })
  #expect(model.layer == .project)
  #expect(!model.editable)
  #expect(store.view?.projectFile == "/project/.cox/config.toml")
  #expect(store.sections.map(\.group) == [.models, .budget, .appearance])
}

@MainActor
@Test func anEditReachesRustAsJsonAndMovesToTheUserLayer() async throws {
  let (store, client) = await loadedStore()
  await store.set("desktop.appearance.opacity", to: .number(0.5))
  await store.set("desktop.appearance.material", to: .text("solid"))
  #expect(
    client.sent == ["desktop.appearance.opacity=0.5", "desktop.appearance.material=\"solid\""])
  let opacity = try #require(store.view?.settings.first { $0.key == "desktop.appearance.opacity" })
  #expect(opacity.layer == .user)
  #expect(store.failure == nil)
}

@MainActor
@Test func aRefusedEditKeepsTheViewAndSaysWhy() async {
  let (store, client) = await loadedStore()
  await store.set("tiers.code.model", to: .text("mine"))
  #expect(client.sent.isEmpty)
  #expect(store.view == fixtureView)
  #expect(store.failure != nil)
}

@MainActor
@Test func aNumberJsonCannotCarryIsNotSent() async {
  let (store, client) = await loadedStore()
  await store.set("desktop.appearance.opacity", to: .number(.nan))
  #expect(client.sent.isEmpty)
  #expect(store.failure != nil)
}

@MainActor
@Test func keysGoToTheSecretStoreForKnownProvidersOnly() async throws {
  let secrets = MemorySecretStore()
  let (store, _) = await loadedStore(secrets: secrets)
  #expect(store.providers == ["anthropic"])
  #expect(!store.hasKey(for: "anthropic"))

  // Rust trims the key and refuses an empty one or an unknown section (T58.4.8); the store
  // stores what it returns and passes a refusal on.
  try store.storeKey("  sk-test \n", for: "anthropic")
  #expect(try secrets.secret(for: "anthropic") == "sk-test")
  #expect(store.hasKey(for: "anthropic"))
  // Observed, so the Settings row that shows the key redraws.
  #expect(store.storedKeys == ["anthropic"])
  #expect(throws: KeyError.empty) { try store.storeKey(" ", for: "anthropic") }
  #expect(throws: KeyError.unknownProvider("nope")) { try store.storeKey("k", for: "nope") }

  try store.removeKey(for: "anthropic")
  #expect(!store.hasKey(for: "anthropic"))
  #expect(store.storedKeys.isEmpty)

  try secrets.store("sk-elsewhere", for: "anthropic")
  await store.load()
  #expect(store.storedKeys == ["anthropic"])
}
