// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `SettingPopUp` (DS§6.3 row `SettingPopUp`, the mockup's Settings `.sel` with its chevron): a
// setting picked from a menu — a tier's model, an enum with more options than a segmented control
// holds. Separate from `CoxSegmented`, which shows every option at once, and from `SettingField`,
// whose well takes typed text.

import SwiftUI

/// The selected option's title and a chevron on a window-surface face with a hairline, lifted to
/// e1; clicking opens a menu of `options` with `selection` checked.
struct SettingPopUp<Option: Hashable>: View {
  let label: LocalizedStringKey
  @Binding var selection: Option
  let options: [Option]
  let title: (Option) -> String

  init(
    _ label: LocalizedStringKey, selection: Binding<Option>, options: [Option],
    title: @escaping (Option) -> String
  ) {
    self.label = label
    self._selection = selection
    self.options = options
    self.title = title
  }

  var body: some View {
    let shape = RoundedRectangle(cornerRadius: Radius.s, style: .continuous)
    Menu {
      Picker(label, selection: $selection) {
        ForEach(options, id: \.self) { Text(title($0)).tag($0) }
      }
      .pickerStyle(.inline)
      .labelsHidden()
    } label: {
      HStack(spacing: Space.s) {
        Text(title(selection)).lineLimit(1)
        Image(systemName: "chevron.down").symbolStyle(.micro)
      }
      .textStyle(.control)
      .foregroundStyle(Color(.textPrimary))
      .padding(.horizontal, Space.m)
      .frame(height: Size.buttonHeightSmall)
      .background(Color(.surfaceWindow), in: shape)
      .hairline(in: shape)
      .elevation(.e1, cornerRadius: Radius.s)
      .contentShape(shape)
    }
    .menuStyle(.button)
    .buttonStyle(.plain)
    .menuIndicator(.hidden)
    .fixedSize()
    .accessibilityLabel(Text(label))
    .accessibilityValue(title(selection))
  }
}

#Preview("pop-up") {
  PreviewMatrix {
    SettingPopUp(
      "Effort", selection: .constant(PreviewState.popUpOptions[2]),
      options: PreviewState.popUpOptions, title: { $0 })
  }
}
