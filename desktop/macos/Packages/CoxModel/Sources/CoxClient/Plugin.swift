// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A plugin's slot as the core reports it (`cox_app::PluginSlot`, PL§8, T52.17): which plugin,
// which slot, and the widget tree cox-app already sanitized and bounded. Separate from the
// timeline's block types because a slot sits beside the blocks, never among them.

/// Where a plugin draws (`cox_protocol::plugin::Slot`).
public enum PluginSlotKind: Hashable, Sendable {
  case statusLeft, statusRight, panel, overlay
}

/// A run of plugin text in one theme role (`cox_app::SpanView`).
public struct PluginRun: Equatable, Sendable {
  public var text: String
  public var token: StyleToken
  public var bold: Bool
  public var italic: Bool

  public init(_ text: String, token: StyleToken = .text, bold: Bool = false, italic: Bool = false) {
    (self.text, self.token, self.bold, self.italic) = (text, token, bold, italic)
  }
}

/// One `key: value` row (`cox_app::KeyValueRow`).
public struct PluginRow: Equatable, Sendable {
  public var key: PluginRun
  public var value: [PluginRun]

  public init(key: PluginRun, value: [PluginRun]) {
    (self.key, self.value) = (key, value)
  }
}

/// PL§8's closed widget tree (`cox_app::WidgetView`). The terminal cell widths and stack sizes
/// are left out: the app lays the tree out natively.
public indirect enum PluginView: Equatable, Sendable {
  case text(lines: [[PluginRun]])
  case list(items: [[PluginRun]], selected: UInt32?)
  case table(header: [PluginRun], rows: [[PluginRun]])
  case keyValue(rows: [PluginRow])
  case gauge(ratio: Double, label: PluginRun)
  case stack(vertical: Bool, children: [PluginView])
  /// `child` is empty or one tree: the FFI carries the recursion as a list.
  case block(title: PluginRun?, child: [PluginView])
}

/// The key a slot patch replaces: one plugin's one slot.
public struct PluginSlotKey: Hashable, Sendable {
  public var plugin: String
  public var slot: PluginSlotKind

  public init(plugin: String, slot: PluginSlotKind) {
    (self.plugin, self.slot) = (plugin, slot)
  }
}

/// One plugin slot's latest state (`cox_app::PluginSlot`).
public struct PluginSlot: Equatable, Sendable {
  public var plugin: String
  public var slot: PluginSlotKind
  /// The last good render; once stopped, the "⚠ <id> slow" line cox-app wrote.
  public var view: PluginView?
  /// Status segments always are; a panel or overlay while it is shown.
  public var visible: Bool
  public var stopped: Bool

  public init(
    plugin: String, slot: PluginSlotKind, view: PluginView?, visible: Bool, stopped: Bool = false
  ) {
    (self.plugin, self.slot, self.view, self.visible, self.stopped) =
      (plugin, slot, view, visible, stopped)
  }

  public var key: PluginSlotKey { PluginSlotKey(plugin: plugin, slot: slot) }
}
