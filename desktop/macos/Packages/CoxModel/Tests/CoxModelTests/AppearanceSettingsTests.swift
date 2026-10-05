// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Appearance popover's path to the config (T37.26): a change it reports writes its
// `[desktop.appearance]` key through the fixture client, and the section reads back from the
// view Rust answers with.

import CoxClient
import Testing

@testable import CoxModel

/// The five `[desktop.appearance]` rows as `cox_app::settings` exports them, at their defaults.
private let appearanceView = SettingsView(
  settings: [
    row("blur", "34.0", .number(min: 0, max: 60)),
    row("depth", "1.0", .number(min: 0, max: 1)),
    row("material", "\"frosted\"", .choice(options: ["frosted", "glossy", "solid"])),
    row("opacity", "0.42", .number(min: 0, max: 1)),
    row("tint", "true", .toggle),
  ],
  userFile: "/home/.cox/config.toml")

private func row(_ field: String, _ value: String, _ kind: SettingKind) -> Setting {
  Setting(
    key: "desktop.appearance.\(field)", value: value, layer: .default, editable: true,
    kind: kind, description: "")
}

@MainActor
private func loaded() async -> Loaded {
  let client = FixtureSettingsClient(view: appearanceView)
  let store = SettingsStore(client: client, secrets: MemorySecretStore(), cwd: "/project")
  await store.load()
  return (store, client)
}

@MainActor
@Test func theSectionReadsBackAsStored() async {
  let (store, _) = await loaded()
  #expect(
    store.appearance
      == DesktopAppearance(
        material: .frosted, opacity: 0.42, blur: 34, blurRange: 0...60, depth: 1, tint: true))
}

@MainActor
@Test func aSliderChangeWritesItsKeyThroughTheClient() async {
  let (store, client) = await loaded()
  await store.apply(.opacity(0.3))
  #expect(client.sent == ["desktop.appearance.opacity=0.3"])
  #expect(store.appearance?.opacity == 0.3)
  #expect(store.failure == nil)
}

@MainActor
@Test func everyEditWritesItsOwnKeyAsJson() async {
  let (store, client) = await loaded()
  for edit in [
    AppearanceEdit.material(.glossy), .blur(12), .depth(0.25), .tint(false),
  ] {
    await store.apply(edit)
  }
  #expect(
    client.sent == [
      "desktop.appearance.material=\"glossy\"", "desktop.appearance.blur=12",
      "desktop.appearance.depth=0.25", "desktop.appearance.tint=false",
    ])
  #expect(
    store.appearance
      == DesktopAppearance(
        material: .glossy, opacity: 0.42, blur: 12, blurRange: 0...60, depth: 0.25, tint: false))
}

@Test func aMissingOrMistypedKeyReadsAsNoSection() {
  #expect(DesktopAppearance(Array(appearanceView.settings.dropFirst())) == nil)
  var mistyped = appearanceView.settings
  mistyped[2].value = "\"chrome\""
  #expect(DesktopAppearance(mistyped) == nil)
}

@MainActor
@Test func theDarkHighlightKeysReadBackFromTheSettings() async {
  let view = SettingsView(
    settings: [
      row("dark_highlight", "\"subtle\"", .choice(options: ["none", "subtle"])),
      row("dark_highlight_scope", "\"all\"", .choice(options: ["controls", "all"])),
    ],
    userFile: "/home/.cox/config.toml")
  let store = SettingsStore(
    client: FixtureSettingsClient(view: view), secrets: MemorySecretStore(), cwd: "/project")
  #expect(store.darkHighlight == .none)
  #expect(store.darkHighlightScope == .controls)
  await store.load()
  #expect(store.darkHighlight == .subtle)
  #expect(store.darkHighlightScope == .all)
  await store.set("desktop.appearance.dark_highlight", to: .text("none"))
  #expect(store.darkHighlight == .none)
}
