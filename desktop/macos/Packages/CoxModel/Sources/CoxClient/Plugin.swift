// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A plugin's slot as the core reports it (`cox_app::PluginSlot`, PL§8, T52.17): which plugin,
// which slot, and the widget tree cox-app already sanitized and bounded. Separate from the
// timeline's block types because a slot sits beside the blocks, never among them.

/// Where a plugin draws (`cox_protocol::plugin::Slot`).
public enum PluginSlotKind: Hashable, Sendable {
  case statusLeft, statusRight, panel, overlay
  /// A plugin's inspector tab (PL§15.4); it renders only while selected.
  case desktopInspector
}

/// A run of plugin text in one theme role (`cox_app::SpanView`).
public struct PluginRun: Equatable, Sendable {
  public var text: String
  public var token: StyleToken
  public var bold: Bool
  public var italic: Bool
  /// What a click opens, already checked by cox-app (PL§15.2); hand it to `openLink`.
  public var link: String?

  public init(
    _ text: String, token: StyleToken = .text, bold: Bool = false, italic: Bool = false,
    link: String? = nil
  ) {
    (self.text, self.token, self.bold, self.italic, self.link) = (text, token, bold, italic, link)
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
  /// A PNG from the plugin's package (PL§15.4); `alt` is its accessibility label.
  case image(alt: String, png: [UInt8])
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

/// A plugin's inspector tab (`cox_app::PluginTab`, PL§15.4).
public struct PluginTab: Equatable, Sendable {
  public var plugin: String
  public var title: String
  /// An SF Symbol name; one the system does not know draws `puzzlepiece.extension`.
  public var symbol: String

  public init(plugin: String, title: String, symbol: String) {
    (self.plugin, self.title, self.symbol) = (plugin, title, symbol)
  }
}

/// A toolbar item (`cox_app::PluginAction`, PL§15.4); `id` is `plugin:<id>:<command>`.
public struct PluginAction: Equatable, Sendable {
  public var id: String
  public var plugin: String
  public var title: String
  public var symbol: String

  public init(id: String, plugin: String, title: String, symbol: String) {
    (self.id, self.plugin, self.title, self.symbol) = (id, plugin, title, symbol)
  }
}

/// What a click on a plugin link did (`cox_app::LinkClick`).
public enum LinkClick: Equatable, Sendable {
  /// The URL went to the host, or to its confirmation for a remote session.
  case opened
  /// A file for the app to show; a local session's path is confined and absolute.
  case file(path: String, line: UInt32?)
  /// Not a link a click may follow.
  case refused
}

/// A plugin's desktop notice for a system notification (`cox_app::PluginAlert`, PL§15.4).
public struct PluginAlert: Equatable, Sendable {
  /// A button: its label and the `plugin:<id>:<command>` it runs.
  public struct Action: Equatable, Sendable {
    public var label: String
    public var id: String

    public init(label: String, id: String) { (self.label, self.id) = (label, id) }
  }

  public var session: String
  public var plugin: String
  public var warn: Bool
  public var title: String
  public var body: String
  public var actions: [Action]

  public init(
    session: String, plugin: String, warn: Bool = false, title: String, body: String,
    actions: [Action] = []
  ) {
    (self.session, self.plugin, self.warn) = (session, plugin, warn)
    (self.title, self.body, self.actions) = (title, body, actions)
  }
}
