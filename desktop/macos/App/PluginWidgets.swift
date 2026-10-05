// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The plugin slots as CoxUI draws them (PL§8, T52.17): CoxClient's `PluginView` mapped onto
// CoxUI's `PluginWidget` through CoxTranscript, which slots the session window shows, and the
// overlay's sheet. Separate because CoxUI imports no cox package. The trees were sanitized
// and bounded in `cox_app::plugin_ui` before they crossed the FFI.

import CoxClient
import CoxModel
import CoxTranscript
import CoxUI
import SwiftUI

enum PluginWidgets {
  /// The toolbar's segments: every `status.left`, then every `status.right`.
  @MainActor static func status(_ store: SessionStore) -> [PluginWidget] {
    (store.pluginViews(.statusLeft) + store.pluginViews(.statusRight)).compactMap(widget)
  }

  /// The panels shown above the composer.
  @MainActor static func panels(_ store: SessionStore) -> [PluginPanelItem] {
    store.pluginViews(.panel).compactMap { slot in
      widget(slot).map { PluginPanelItem(plugin: slot.plugin, widget: $0) }
    }
  }

  /// The overlay shown, if any; the core shows at most one.
  @MainActor static func overlay(_ store: SessionStore) -> PluginWidget? {
    store.pluginViews(.overlay).first.flatMap(widget)
  }

  /// `size` in cells of the plugin text's font (`monoCode`): the area `cox_render` offers a
  /// panel or overlay, as a terminal's cells are the TUI's (PL§8).
  static func cells(_ size: CGSize, scale: Double) -> (width: UInt16, height: UInt16) {
    let font = FontToken.monoCode.nsFont(scale: scale)
    let cell = ("M" as NSString).size(withAttributes: [.font: font])
    let line = NSLayoutManager().defaultLineHeight(for: font)
    guard cell.width > 0, line > 0 else { return (0, 0) }
    return (
      UInt16(clamping: Int(size.width / cell.width)), UInt16(clamping: Int(size.height / line))
    )
  }

  private static func widget(_ slot: PluginSlot) -> PluginWidget? {
    slot.view.map(PluginWidget.init)
  }
}

extension View {
  /// A plugin overlay as a sheet while the core says it is shown; Esc, which dismisses the sheet,
  /// hides it in the core too, so the next patch agrees.
  func pluginOverlaySheet(_ store: SessionStore) -> some View {
    let isShown = Binding(
      get: { PluginWidgets.overlay(store) != nil },
      set: { if !$0 { store.closePluginOverlay() } })
    return sheet(isPresented: isShown) {
      if let overlay = PluginWidgets.overlay(store) {
        ScrollView { PluginWidgetView(overlay).padding(Space.xl) }
          .frame(minWidth: Size.readingWidth, minHeight: Size.popoverWidth)
      }
    }
  }
}
