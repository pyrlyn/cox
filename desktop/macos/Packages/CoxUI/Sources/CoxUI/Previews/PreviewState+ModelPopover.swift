// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the model popover (T37.22.6): the default config's tiers as the
// core lists them, with claude-sonnet-5 running, and the main screen with it open under the capsule.
// Separate from the other fixture files so this card adds its own.

extension PreviewState {
  static let models = ModelPopover.State(sections: [
    ModelPopover.Section(
      title: "Code",
      rows: [
        CompletionList.Row(
          id: "code/claude-sonnet-5", title: "claude-sonnet-5", detail: "low · high"),
        CompletionList.Row(id: "code/claude-haiku-4-5", title: "claude-haiku-4-5", detail: ""),
      ], selected: "code/claude-sonnet-5"),
    ModelPopover.Section(
      title: "Think",
      rows: [
        CompletionList.Row(
          id: "think/claude-fable-5-1", title: "claude-fable-5-1", detail: "low · high · xhigh")
      ]),
  ])

  /// The main screen with the model popover open.
  static var modelOpen: MainScreenState {
    var state = main
    state.toolbar.popover = .model
    state.model = models
    return state
  }
}
