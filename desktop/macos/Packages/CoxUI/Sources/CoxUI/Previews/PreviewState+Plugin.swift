// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for `PluginWidgetView` (T52.16, PL§8): one tree per widget variant, a
// status-segment-sized line and a block nesting the others, as a plugin's panel would. Separate
// so the plugin fixtures change without touching another surface's.

import SwiftUI

extension PreviewState {
  /// A named sample, for a preview row and a snapshot name.
  struct PluginSample: Sendable {
    let name: String
    let widget: PluginWidget
  }

  static let pluginText = PluginWidget.text([
    [.init("3 turns · ", .dim), .init("$0.42", .accent, isBold: true)],
    [.init("cache hit 81%", .ok), .init(" · ", .dim), .init("retrying", .warn, isItalic: true)],
  ])

  static let pluginList = PluginWidget.list(
    items: [
      [.init("retry.rs")], [.init("http.rs"), .init("  modified", .dim)], [.init("sse.rs")],
    ],
    selected: 1)

  static let pluginTable = PluginWidget.table(
    header: [.init("Crate", .dim), .init("Tests", .dim), .init("Status", .dim)],
    rows: [
      [.init("cox-app"), .init("41"), .init("pass", .ok)],
      [.init("cox-ffi"), .init("3"), .init("fail", .error)],
    ])

  static let pluginKeyValue = PluginWidget.keyValue([
    .init(key: .init("branch", .dim), value: [.init("main", .accent)]),
    .init(key: .init("diff", .dim), value: [.init("+12", .diffAdd), .init(" −3", .diffDel)]),
  ])

  static let pluginGauge = PluginWidget.gauge(ratio: 0.62, label: .init("context", .dim))

  static let pluginStack = PluginWidget.stack(vertical: false, children: [pluginGauge, pluginText])

  /// A panel's tree: a titled block over a stack of the other variants.
  static let pluginBlock = PluginWidget.block(
    title: .init("acme", .accent, isBold: true),
    child: .stack(vertical: true, children: [pluginKeyValue, pluginList, pluginTable]))

  static let pluginWidgets = [
    PluginSample(name: "text", widget: pluginText),
    PluginSample(name: "list", widget: pluginList),
    PluginSample(name: "table", widget: pluginTable),
    PluginSample(name: "keyValue", widget: pluginKeyValue),
    PluginSample(name: "gauge", widget: pluginGauge),
    PluginSample(name: "stack", widget: pluginStack),
    PluginSample(name: "block", widget: pluginBlock),
  ]

  /// Two status segments, the second longer than a segment's width, so it truncates (T52.17).
  static let pluginStatus = [
    PluginWidget.text([[.init("main", .accent), .init(" +12", .diffAdd)]]),
    PluginWidget.text([[.init("cache hit 81% · 3 retries · last sync 2 min ago", .dim)]]),
  ]

  /// Two shown panels, the first taller than a panel's eight rows, so it scrolls (T52.17).
  static let pluginPanels = [
    PluginPanelItem(plugin: "acme", widget: pluginBlock),
    PluginPanelItem(plugin: "usage", widget: pluginStack),
  ]
}
