// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the setting and figure molecules (T37.21): the Appearance
// popover's switches and scales and the token popover's totals, as mockup screens 19, 28 and
// 30 show them. Separate from `PreviewState.swift` so molecules built in parallel add their
// fixtures without editing one file.

import SwiftUI

extension PreviewState {
  static let toggleTitle = "Allow Bypass mode"
  static let toggleDetail = "shows a red strip while on; shell stays sandboxed"

  static let sliderTitle = "Window transparency"
  static let sliderValue = 0.58
  static let sliderText = "58%"
  static let sliderEnds = (low: "Opaque", high: "Clear")

  static let tokenColumns = ["Turn", "Session"]
  /// The token popover's totals for one turn.
  static let tokenRows: [KeyValueGrid.Row] = [
    .init(label: "↑ Sent", values: ["41.6k", "218.5k"]),
    .init(label: "cache read", values: ["38.9k", "152.3k"], isDetail: true),
    .init(label: "cache write", values: ["1.1k", "34.0k"], isDetail: true),
    .init(label: "uncached", values: ["1.6k", "32.2k"], isDetail: true),
    .init(label: "↓ Received", values: ["1.9k", "9.8k"]),
    .init(label: "of it thinking", values: ["0.6k", "3.1k"], isDetail: true),
    .init(label: "Cost", values: ["$0.05", "$0.42"]),
  ]

  /// Facts as the inspector lists them: one value per label.
  static let factRows: [KeyValueGrid.Row] = [
    .init(label: "Model", values: ["Sonnet 5 · high"]),
    .init(label: "Requests", values: ["4"]),
    .init(label: "First token", values: ["0.8 s"]),
  ]
}

/// The Bypass switch, on or off, with or without its detail line, at the popover's width.
struct LabeledToggleSample: View {
  let isOn: Bool
  let detail: String?

  var body: some View {
    LabeledToggle(PreviewState.toggleTitle, detail: detail, isOn: .constant(isOn))
      .frame(width: Size.popoverWidth)
  }
}

/// The window-transparency scale, with or without its ends, at the popover's width.
struct LabeledSliderSample: View {
  let ends: (low: String, high: String)?

  var body: some View {
    LabeledSlider(
      PreviewState.sliderTitle, value: .constant(PreviewState.sliderValue),
      valueText: PreviewState.sliderText, ends: ends
    )
    .frame(width: Size.popoverWidth)
  }
}
