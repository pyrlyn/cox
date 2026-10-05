// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `SessionFilter` (DS§6.3 row `SessionFilter`, the mockup's `.filter`): the search field over
// the sidebar's sessions (and Settings' search), with the shortcut that focuses it. Separate so
// every filter field is the same sunken well with the same hint.

import SwiftUI

/// A magnifier, the text field and a `KeyCap` in a sunken `fill.primary` well.
struct SessionFilter: View {
  @Binding var text: String
  let prompt: String
  /// The shortcut that focuses the field, `⌘K`, or `nil` for none.
  let shortcut: String?

  init(text: Binding<String>, prompt: String, shortcut: String? = nil) {
    self._text = text
    self.prompt = prompt
    self.shortcut = shortcut
  }

  var body: some View {
    HStack(spacing: Space.s) {
      Image(systemName: "magnifyingglass")
        .symbolStyle(.body)
        .foregroundStyle(Color(.textPlaceholder))
        .accessibilityHidden(true)
      // The prompt and its magnifier in `text.placeholder`, not the mockup's tertiary, so the hint
      // holds 4.5:1 on glass (DS§8, A112); the label names it for VoiceOver.
      TextField(prompt, text: $text, prompt: Text(prompt).foregroundStyle(Color(.textPlaceholder)))
        .textFieldStyle(.plain)
        .textStyle(.body)
        .foregroundStyle(Color(.textPrimary))
      if let shortcut { KeyCap(shortcut) }
    }
    .padding(.horizontal, Space.ml)
    .frame(height: Size.buttonHeight)
    .insetWell(Color(.fillPrimary), cornerRadius: Radius.m)
  }
}

#Preview("empty") {
  PreviewMatrix {
    SessionFilter(
      text: .constant(""), prompt: PreviewState.filterPrompt, shortcut: PreviewState.keys
    )
    .frame(width: Size.sidebarWidth)
  }
}
#Preview("typed") {
  PreviewMatrix {
    SessionFilter(text: .constant(PreviewState.filterText), prompt: PreviewState.filterPrompt)
      .frame(width: Size.sidebarWidth)
  }
}
