// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for `SettingRow` (T37.21.10): Settings rows from each source layer,
// as mockup screen 12 shows them. Separate from `PreviewState+Settings.swift` so molecules
// built in parallel add their fixtures without editing one file.

import SwiftUI

extension PreviewState {
  static let settingTitle = "Default mode"
  static let settingDetail = "for new sessions"
}

/// One Settings row per shape, at the reading column's width, as a Settings group lays them.
enum SettingRowSample {
  /// A switch the user's config set.
  static var toggle: some View {
    SettingRow(source: .user) {
      LabeledToggle(
        PreviewState.toggleTitle, detail: PreviewState.toggleDetail, isOn: .constant(false))
    }
    .frame(width: Size.readingWidth)
  }

  /// A titled control left at its default.
  static var control: some View {
    SettingRow(PreviewState.settingTitle, detail: PreviewState.settingDetail, source: .default) {
      ModeSegmented(selection: .constant(.ask))
    }
    .frame(width: Size.readingWidth)
  }

  /// A scale the user's config set.
  static var slider: some View {
    SettingRow(source: .user) {
      LabeledSlider(
        PreviewState.sliderTitle, value: .constant(PreviewState.sliderValue),
        valueText: PreviewState.sliderText, ends: PreviewState.sliderEnds)
    }
    .frame(width: Size.readingWidth)
  }

  /// The same control locked by the project's config.
  static var readOnly: some View {
    SettingRow(PreviewState.settingTitle, detail: PreviewState.settingDetail, source: .project) {
      ModeSegmented(selection: .constant(.plan))
    }
    .frame(width: Size.readingWidth)
  }
}
