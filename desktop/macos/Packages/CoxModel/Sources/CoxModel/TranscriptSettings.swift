// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `[desktop.transcript]` (A67, A93) as the transcript reads it: whether a drag runs across
// blocks, and the prose size and line height the app hands `TranscriptView`. Read back from
// the settings view like `[desktop.appearance]`, so a Settings edit or `cox config set`
// restyles the open transcript without a second config reader. Separate because the
// Appearance popover edits the other section, not this one.

import CoxClient
import Foundation

/// `[desktop.transcript]` as Rust stored it.
public struct DesktopTranscript: Equatable, Sendable {
  public var crossBlockSelection: Bool
  /// The prose size in pt at 100 % text size; ⌘+/⌘− scale it.
  public var textSize: Double
  /// The prose line height, as a multiple of its size.
  public var lineHeight: Double
}

extension DesktopTranscript {
  /// `nil` when a key is missing or holds a value of another type.
  init?(_ settings: [Setting]) {
    let rows = SectionRows(settings, "desktop.transcript")
    guard let cross: Bool = rows.decode("cross_block_selection"),
      let size: Double = rows.decode("text_size"), let line: Double = rows.decode("line_height")
    else { return nil }
    (crossBlockSelection, textSize, lineHeight) = (cross, size, line)
  }
}

extension SettingsStore {
  /// `[desktop.transcript]` from the loaded view; `nil` before the first load.
  public var transcript: DesktopTranscript? {
    view.flatMap { DesktopTranscript($0.settings) }
  }
}

/// One config section's rows by field, as the settings view holds them.
struct SectionRows {
  private let section: String
  private let rows: [String: Setting]

  init(_ settings: [Setting], _ section: String) {
    self.section = section
    rows = Dictionary(settings.map { ($0.key, $0) }) { first, _ in first }
  }

  subscript(_ field: String) -> Setting? { rows["\(section).\(field)"] }

  /// The field's JSON value; `nil` when it is missing or holds a value of another type.
  func decode<Value: Decodable>(_ field: String) -> Value? {
    self[field].flatMap { try? JSONDecoder().decode(Value.self, from: Data($0.value.utf8)) }
  }
}
