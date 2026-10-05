// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PluginPanel` (DS§6.4 row `PluginPanel`, PL§8, T52.17): the plugin `panel` slot above the
// composer — each shown panel under a header naming its plugin, at most eight rows of the code
// face tall, scrolling past that. Separate so the session column shows plugin output from plain
// values and CoxUI imports no cox package; the app decides which panels are shown.

import SwiftUI

/// One shown panel: its plugin's id and its widget tree.
public struct PluginPanelItem: Equatable, Sendable, Identifiable {
  public var plugin: String
  public var widget: PluginWidget

  public var id: String { plugin }

  public init(plugin: String, widget: PluginWidget) {
    (self.plugin, self.widget) = (plugin, widget)
  }
}

/// The header on `fill.primary` under a top hairline, `font.caption` in `text.secondary`, as the
/// terminal pane's; below, the tree on `surface.window`.
public struct PluginPanel: View {
  let items: [PluginPanelItem]

  public init(_ items: [PluginPanelItem]) {
    self.items = items
  }

  /// PL§8's `PANEL_ROWS`: the rows a panel may take before it scrolls.
  private static let rows: CGFloat = 8

  /// Eight lines of `font.mono.code` at its line height.
  private static var maxHeight: CGFloat {
    FontToken.monoCode.size * FontToken.monoCode.lineHeight * rows
  }

  public var body: some View {
    VStack(spacing: 0) {
      ForEach(items) { item in
        Text(item.plugin)
          .textStyle(.caption)
          .foregroundStyle(Color(.textSecondary))
          .lineLimit(1)
          .padding(.horizontal, Space.l)
          .padding(.vertical, Space.xs)
          .frame(maxWidth: .infinity, alignment: .leading)
          .background(Color(.fillPrimary))
          .hairline(.top)
        ScrollView(.vertical) {
          PluginWidgetView(item.widget)
            .padding(.horizontal, Space.l)
            .padding(.vertical, Space.s)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .scrollBounceBehavior(.basedOnSize)
        .frame(maxHeight: Self.maxHeight)
        .background(Color(.surfaceWindow))
      }
    }
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Plugin panels")
  }
}

#Preview("panel") {
  PreviewMatrix {
    PluginPanel([PluginPanelItem(plugin: "git-status", widget: PreviewState.pluginBlock)])
      .frame(width: Size.readingWidth)
  }
}
