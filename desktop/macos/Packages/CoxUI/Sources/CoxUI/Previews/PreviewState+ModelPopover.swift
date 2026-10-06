// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the model popover (T37.22.6): the default config's tiers as the
// core lists them, with claude-sonnet-5 running, and the main screen with it open under the capsule;
// and, for the provider in the chip (T60.7), the providers' marks, the chip's states and a popover
// of two providers, one without a key. Separate from the other fixture files so this card adds
// its own.

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

  static let provider = ProviderMark(slug: "anthropic", name: "Anthropic")
  static let localProvider = ProviderMark(slug: "lmstudio", name: "LM Studio")

  /// The composer's model chip over a ready provider.
  static var composerProviderReady: Composer.State {
    var state = composerStatus
    (state.model, state.provider) = ("Sonnet 5 · high", provider)
    return state
  }

  /// The same chip when the provider has no key: the badge and its tooltip's text.
  static var composerProviderProblem: Composer.State {
    var state = composerProviderReady
    state.modelProblem = "No API key for anthropic. Add one in Settings, or pick another provider."
    return state
  }

  /// Anthropic's models, in use, and OpenAI's, which has no key: greyed, with "Add key".
  static let modelsOfTwoProviders = ModelPopover.State(sections: [
    ModelPopover.Section(
      title: "Code",
      rows: [
        CompletionList.Row(
          id: "code/anthropic/claude-sonnet-5", title: "Sonnet 5", detail: "low · high"),
        CompletionList.Row(id: "code/anthropic/claude-haiku-4-5", title: "Haiku 4.5", detail: ""),
      ], selected: "code/anthropic/claude-sonnet-5", provider: "anthropic"),
    ModelPopover.Section(
      title: "OpenAI",
      rows: [CompletionList.Row(id: "code/openai/gpt-5-5", title: "GPT-5.5", detail: "low · high")],
      provider: "openai", isEnabled: false, offersKey: true),
  ])
}
