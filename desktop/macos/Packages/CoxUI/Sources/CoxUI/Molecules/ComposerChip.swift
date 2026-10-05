// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ComposerChip` (DS§6.3 row `ComposerChip`, the mockup's `.chip` and `.chip.blue`): one thing
// the next message carries besides its text — an @-mentioned file, an attachment, a slash
// command, shell mode, the prompts queued behind the running turn, the permission mode, the
// model it runs under and the think toggle — with a way to take it back out. Separate so the composer shows everything
// it will send as the same small lifted pill.

import SwiftUI

/// The kind's symbol if it has one, the label in `font.caption`, an optional `KeyCap`, and an `xmark` that
/// removes the chip, on a readable capsule face lifted to e1 (DS§3.4). Mentions and commands
/// change what the model sees, and queued prompts wait on it, so they are tinted `accent`; a
/// mode takes its DS§3.1 colour, as `ModeSegmented` shows it; think is tinted `accent` while on.
struct ComposerChip: View {
  enum Kind: Equatable, Sendable, CaseIterable {
    case mention, attachment, command, shell, queued, model
    case mode(SessionMode)
    /// The think toggle, on or off (A103).
    case think(Bool)

    static let allCases: [Kind] =
      [.mention, .attachment, .command, .shell, .queued, .model, .think(false), .think(true)]
      + SessionMode.allCases.map(mode)
  }

  let label: String
  let kind: Kind
  /// The shortcut that toggles the chip, `⇧⇥`, or `nil`.
  let shortcut: String?
  /// Takes the chip out of the message; `nil` for a chip that cannot be removed.
  let onRemove: (() -> Void)?

  init(
    _ label: String, kind: Kind, shortcut: String? = nil, onRemove: (() -> Void)? = nil
  ) {
    self.label = label
    self.kind = kind
    self.shortcut = shortcut
    self.onRemove = onRemove
  }

  var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.capsule, style: .continuous)
    // The mockup's 5 px gap takes the nearest step, `Space.xs`.
    HStack(spacing: Space.xs) {
      if let symbol = kind.symbol { Image(systemName: symbol).symbolStyle(.caption) }
      // An empty label leaves the symbol alone, the paperclip's chip.
      if !label.isEmpty {
        Text(label)
          .textStyle(.caption)
          .lineLimit(1)
          .truncationMode(.middle)
      }
      if let shortcut { KeyCap(shortcut) }
      if let onRemove { RemoveButton(label: label, action: onRemove) }
    }
    .foregroundStyle(kind.foreground)
    .padding(.horizontal, Space.ml)
    .frame(height: Size.chipHeight)
    // Face behind the label, so the glass sweep never washes out the text (DS§8).
    .background {
      shape.fill(kind.tint).glassPane(shape, surface: Color(.surfaceCapsule), role: .readable)
    }
    .hairline(in: shape)
    .elevation(.e1, cornerRadius: Radius.capsule)
    .accessibilityElement(children: .contain)
  }
}

/// The chip's `xmark`: a bare glyph, named for VoiceOver and the tooltip after what it removes.
private struct RemoveButton: View {
  let label: String
  let action: () -> Void

  var body: some View {
    Button(action: action) {
      Image(systemName: "xmark")
        .symbolStyle(.micro)
        .foregroundStyle(Color(.textTertiary))
        .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .help("Remove \(label)")
    .accessibilityLabel("Remove \(label)")
  }
}

/// The think toggle (A103): the chip that sends the next turn to the think tier, tinted while on;
/// the store turns it off once that turn is sent.
struct ThinkChip: View {
  let isOn: Bool
  let toggle: () -> Void

  var body: some View {
    Button(action: toggle) {
      ComposerChip("Think", kind: .think(isOn))
    }
    .buttonStyle(.plain)
    .help(
      isOn
        ? "The next message goes to the think tier · click to turn it off"
        : "Send the next message to the think tier"
    )
    .accessibilityValue(isOn ? "On" : "Off")
  }
}

extension ComposerChip.Kind {
  /// The mockup draws the mode and the queue as bare labels.
  var symbol: String? {
    switch self {
    case .mention: "at"
    case .attachment: "paperclip"
    case .command: "bolt"
    case .shell: "terminal"
    case .queued, .mode: nil
    case .model: "sparkle"
    case .think: "brain"
    }
  }

  var foreground: Color {
    switch self {
    case .mention, .command, .queued, .mode(.auto), .think(true): Color(.accent)
    case .attachment, .shell, .model, .mode(.ask), .think(false): Color(.textSecondary)
    case .mode(.plan): Color(.statusPlan)
    case .mode(.bypass): Color(.statusDanger)
    }
  }

  /// The mockup's `.chip.blue` tint over the capsule face.
  var tint: Color {
    switch self {
    case .mention, .command, .queued, .mode(.auto), .think(true): Color(.accentSoft)
    case .mode(.bypass): Color(.statusDangerSoft)
    case .attachment, .shell, .model, .mode(.ask), .mode(.plan), .think(false): .clear
    }
  }
}

#Preview("mention") { PreviewMatrix { ComposerChipSample(kind: .mention) } }
#Preview("attachment") { PreviewMatrix { ComposerChipSample(kind: .attachment) } }
#Preview("command") { PreviewMatrix { ComposerChipSample(kind: .command) } }
#Preview("shell") { PreviewMatrix { ComposerChipSample(kind: .shell) } }
#Preview("queued") { PreviewMatrix { ComposerChipSample(kind: .queued) } }
#Preview("think off") { PreviewMatrix { ComposerChipSample(kind: .think(false)) } }
#Preview("think on") { PreviewMatrix { ComposerChipSample(kind: .think(true)) } }
#Preview("shortcut") { PreviewMatrix { ComposerChipSample.shortcut } }
