// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `SettingRow` (DS§6.3 row `SettingRow`, the mockup's Settings `.group .gr`): one setting in a
// Settings group — its name, its control and the config layer its value comes from. Separate
// so every setting says where its value is set, and one the user's config cannot change is
// shown as read-only rather than as a control that silently does nothing.

import SwiftUI

/// A labelled control (`LabeledToggle`, `LabeledSlider`, or a `SettingLabel` beside any
/// control), then a `Badge` naming the source layer. A value set by a layer above the user's
/// config locks the control and shows why.
struct SettingRow<Content: View>: View {
  let source: SettingSource
  let content: Content

  init(source: SettingSource, @ViewBuilder content: () -> Content) {
    self.source = source
    self.content = content()
  }

  var body: some View {
    HStack(spacing: Space.l) {
      content
        .frame(maxWidth: .infinity, alignment: .leading)
        .disabled(source.isReadOnly)
      HStack(spacing: Space.xs) {
        if source.isReadOnly {
          Image(systemName: "lock.fill")
            .symbolStyle(.micro)
            .foregroundStyle(Color(.textSecondary))
        }
        Badge(source.name, kind: source.badge)
      }
      .help(source.note)
      .accessibilityElement(children: .ignore)
      .accessibilityLabel(source.note)
    }
    // The mockup's 14 px sides fall between `Space.l` and `Space.xl`; the smaller matches the
    // other rows' insets.
    .padding(.horizontal, Space.l)
    .padding(.vertical, Space.ml)
  }
}

extension SettingRow {
  /// A row that names its setting itself: `title` and `detail` beside `control`.
  init<Control: View>(
    _ title: String, detail: String? = nil, source: SettingSource,
    @ViewBuilder control: () -> Control
  ) where Content == TitledSetting<Control> {
    self.init(source: source) { TitledSetting(title: title, detail: detail, control: control()) }
  }
}

/// A `SettingLabel` filling the row, then the control at its natural size.
struct TitledSetting<Control: View>: View {
  let title: String
  let detail: String?
  /// `SettingLabel.namesItem`.
  var namesItem = false
  let control: Control

  var body: some View {
    HStack(spacing: Space.l) {
      SettingLabel(title, detail: detail, namesItem: namesItem)
        .frame(maxWidth: .infinity, alignment: .leading)
      control
    }
  }
}

/// The config layers a setting's value can come from, as `cox config show --sources` names
/// them. Outside `SettingRow` so it does not depend on the row's content type.
public enum SettingSource: CaseIterable, Sendable {
  case `default`, user, project, claudeSettings, env, flag
}

extension SettingSource {
  /// Layers above the user's config win over it, so Settings cannot change what they set; the
  /// Claude settings are imported read-only (plan.md D13).
  var isReadOnly: Bool {
    switch self {
    case .default, .user: false
    case .project, .claudeSettings, .env, .flag: true
    }
  }

  var name: String {
    switch self {
    case .default: "default"
    case .user: "user"
    case .project: "project"
    case .claudeSettings: "claude-settings"
    case .env: "env"
    case .flag: "flag"
    }
  }

  var badge: Badge.Kind {
    switch self {
    case .default: .default
    case .user: .user
    case .project: .project
    case .env: .env
    case .claudeSettings, .flag: .neutral
    }
  }

  /// The badge's tooltip and VoiceOver label.
  var note: String {
    isReadOnly ? "Set by the \(name) layer; change it there" : "Set by the \(name) layer"
  }
}

#Preview("toggle") { PreviewMatrix { SettingRowSample.toggle } }
#Preview("control") { PreviewMatrix { SettingRowSample.control } }
#Preview("slider") { PreviewMatrix { SettingRowSample.slider } }
#Preview("read-only") { PreviewMatrix { SettingRowSample.readOnly } }
