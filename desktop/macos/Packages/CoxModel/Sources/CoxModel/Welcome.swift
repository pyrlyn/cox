// A welcome suggestion's click (Figma frame 22-empty-session): its prompt becomes the composer's
// draft. The facts and their seam are CoxClient's (`Welcome.swift` there), read from cox-app
// (T37.49); this is the one thing the composer does with them.

import CoxClient

extension ComposerStore {
  /// A welcome suggestion's click: its prompt becomes the draft, to read and send.
  public func suggest(_ suggestion: WelcomeSuggestion) { edit(suggestion.prompt) }
}
