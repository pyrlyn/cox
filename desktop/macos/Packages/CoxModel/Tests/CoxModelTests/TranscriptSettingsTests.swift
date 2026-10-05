// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `[desktop.transcript]` reaches the transcript (T37.23.13): the text size and line height the
// settings view holds read back as the section the app hands `TranscriptView`, and an edit
// through the fixture client changes what it reads.

import CoxClient
import Testing

@testable import CoxModel

/// The three `[desktop.transcript]` rows as `cox_app::settings` exports them, off the defaults.
private let transcriptView = SettingsView(
  settings: [
    row("cross_block_selection", "false", .toggle),
    row("line_height", "1.8", .number(min: 1, max: 2.5)),
    row("text_size", "16.0", .number(min: 10, max: 24)),
  ],
  userFile: "/home/.cox/config.toml")

private func row(_ field: String, _ value: String, _ kind: SettingKind) -> Setting {
  Setting(
    key: "desktop.transcript.\(field)", value: value, layer: .user, editable: true, kind: kind,
    description: "")
}

@MainActor
@Test func theTextSizeAndLineHeightReadBackAsStored() async {
  let store = SettingsStore(
    client: FixtureSettingsClient(view: transcriptView), secrets: MemorySecretStore(),
    cwd: "/project")
  #expect(store.transcript == nil)
  await store.load()
  #expect(
    store.transcript
      == DesktopTranscript(crossBlockSelection: false, textSize: 16, lineHeight: 1.8))

  await store.set("desktop.transcript.text_size", to: .number(12))
  #expect(store.transcript?.textSize == 12)
}

@Test func aMissingOrMistypedKeyReadsAsNoTranscriptSection() {
  #expect(DesktopTranscript(Array(transcriptView.settings.dropLast())) == nil)
  var mistyped = transcriptView.settings
  mistyped[1].value = "\"tall\""
  #expect(DesktopTranscript(mistyped) == nil)
}
