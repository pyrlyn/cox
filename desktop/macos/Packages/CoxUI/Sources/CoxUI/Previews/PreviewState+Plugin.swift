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

  /// A line with a file link and a web link, as a plugin points the reader somewhere (PL§15.4).
  static let pluginLink = PluginWidget.text([
    [
      .init("failed in ", .dim), .init("src/http.rs:42", .accent, link: "src/http.rs:42"),
      .init(" · ", .dim), .init("docs", .accent, link: "https://example.com/docs"),
    ]
  ])

  /// A checkerboard drawn here rather than shipped as a file, so the sample needs no resource and
  /// draws the same on every macOS.
  static let pluginImage = PluginWidget.image(alt: "build graph", png: checkerboard(side: 24))

  private static func checkerboard(side: Int) -> Data {
    guard
      let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: side, pixelsHigh: side, bitsPerSample: 8,
        samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
        bytesPerRow: 0, bitsPerPixel: 0)
    else { return Data() }
    let cell = side / 4
    // Fixed greys, not tokens: the bytes are made once, before any appearance applies. Drawn
    // through a context because `setColor(_:atX:y:)` leaves a device-RGB rep's pixels clear.
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    NSColor.lightGray.setFill()
    NSRect(x: 0, y: 0, width: side, height: side).fill()
    NSColor.darkGray.setFill()
    for col in 0..<4 {
      for row in 0..<4 where (col + row).isMultiple(of: 2) {
        NSRect(x: col * cell, y: row * cell, width: cell, height: cell).fill()
      }
    }
    NSGraphicsContext.restoreGraphicsState()
    return rep.representation(using: .png, properties: [:]) ?? Data()
  }

  static let pluginWidgets = [
    PluginSample(name: "text", widget: pluginText),
    PluginSample(name: "list", widget: pluginList),
    PluginSample(name: "table", widget: pluginTable),
    PluginSample(name: "keyValue", widget: pluginKeyValue),
    PluginSample(name: "gauge", widget: pluginGauge),
    PluginSample(name: "stack", widget: pluginStack),
    PluginSample(name: "block", widget: pluginBlock),
    PluginSample(name: "link", widget: pluginLink),
    PluginSample(name: "image", widget: pluginImage),
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

  /// A plugin's inspector tab, selected, after a second plugin's tab whose symbol this macOS does
  /// not ship, so the fallback glyph shows (PL§15.4).
  static let pluginTabs = [
    PluginTabItem(plugin: "acme", title: "Acme", symbol: "chart.bar"),
    PluginTabItem(plugin: "later", title: "Later", symbol: "not.a.symbol"),
  ]

  @MainActor static var pluginInspector: some View {
    Inspector(
      selection: .plugin("acme"), pluginTabs: pluginTabs,
      content: PluginInspectorTab(title: "Acme", widget: pluginBlock)
    ) { _ in }
  }

  /// The bar with a status segment and two toolbar items, the second on the fallback glyph.
  static let pluginToolbar = SessionToolbar.State(
    title: "Fix the retry loop", project: "cox", cost: "$0.42", context: "38%",
    contextFraction: 0.38, pluginStatus: [pluginStatus[0]],
    pluginActions: [
      .init(id: "plugin:acme:sync", title: "Sync", symbol: "arrow.triangle.2.circlepath"),
      .init(id: "plugin:acme:later", title: "Later", symbol: "not.a.symbol"),
    ])
}
