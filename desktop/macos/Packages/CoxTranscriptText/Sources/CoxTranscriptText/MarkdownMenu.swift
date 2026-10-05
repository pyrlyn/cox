// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// "Copy as Markdown" in the transcript's context menu (T37.42.2, DT§5.2):
// the selection's Markdown, as `MarkdownCopy` builds it, as both the Markdown
// and the plain-text type, so a plain editor pastes Markdown too. Its own
// file because it hooks only the menu; ⌘C keeps writing both forms.

import AppKit

extension TranscriptTextView {
  override public func menu(for event: NSEvent) -> NSMenu? {
    let menu = super.menu(for: event) ?? NSMenu()
    let item = NSMenuItem(
      title: "Copy as Markdown", action: #selector(copyAsMarkdown(_:)), keyEquivalent: "")
    item.target = self
    let copy = menu.items.firstIndex { $0.action == #selector(NSText.copy(_:)) }
    menu.insertItem(item, at: copy.map { $0 + 1 } ?? 0)
    return menu
  }

  @objc public func copyAsMarkdown(_ sender: Any?) {
    let markdown = copiedSelection().markdown
    guard !markdown.isEmpty else { return }
    markdownPasteboard.declareTypes([.markdown, .string], owner: nil)
    markdownPasteboard.setString(markdown, forType: .markdown)
    markdownPasteboard.setString(markdown, forType: .string)
  }

  override public func validateMenuItem(_ item: NSMenuItem) -> Bool {
    guard item.action == #selector(copyAsMarkdown(_:)) else { return super.validateMenuItem(item) }
    return selectedRanges.contains { $0.rangeValue.length > 0 }
  }
}
