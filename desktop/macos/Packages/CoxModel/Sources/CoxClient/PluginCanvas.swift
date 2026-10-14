// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A plugin's canvas as the core reports it (`cox_app::PluginCanvas`, docs/design/plugin-canvas.md
// §3, §5, T33.47.8.1): the pane's heading, what one draw changed, and what a click attaches.
// cox-app capped and sanitized each before it crossed the FFI; a frame's `html` stays untrusted
// and goes to CoxPlatform's isolated `CanvasPage` only.
//
// `CanvasClient` is its own protocol rather than more `SessionClient` requirements: a session
// that has canvases conforms to both, and one that has none (a fixture without them, an
// external agent's) needs no stubs.

/// A canvas a plugin declared (`cox_app::PluginCanvas`).
public struct PluginCanvas: Equatable, Sendable {
  public var plugin: String
  public var title: String
  /// An SF Symbol name; one the system does not know draws `puzzlepiece.extension`.
  public var symbol: String
  /// The attribute a click reads, `data-` and lowercase, checked by the plugin API.
  public var selectAttr: String

  public init(plugin: String, title: String, symbol: String, selectAttr: String) {
    (self.plugin, self.title, self.symbol, self.selectAttr) = (plugin, title, symbol, selectAttr)
  }
}

/// One page on a canvas (`cox_plugin_api::Frame`).
public struct CanvasFrame: Equatable, Sendable, Identifiable {
  /// Unique on its canvas; the app hands it back with a click.
  public var id: String
  public var title: String
  public var widthPt: UInt32
  /// Untrusted: shown only through `CanvasPage`.
  public var html: String

  public init(id: String, title: String, widthPt: UInt32, html: String) {
    (self.id, self.title, self.widthPt, self.html) = (id, title, widthPt, html)
  }
}

/// What one draw changed (`cox_app::CanvasUpdate`): `order` is every frame now on the canvas,
/// `frames` only the new or changed ones; `stopped` once the plugin missed three draws in a row.
public struct CanvasUpdate: Equatable, Sendable {
  public var plugin: String
  public var order: [String]
  public var frames: [CanvasFrame]
  public var stopped: Bool

  public init(plugin: String, order: [String], frames: [CanvasFrame], stopped: Bool = false) {
    (self.plugin, self.order, self.frames, self.stopped) = (plugin, order, frames, stopped)
  }
}

/// What a click gives the composer (`cox_plugin_api::CanvasAttach`): a chip named `name`
/// carrying `text` as a `text/plain` attachment.
public struct CanvasAttach: Equatable, Sendable {
  public var name: String
  public var text: String

  public init(name: String, text: String) { (self.name, self.text) = (name, text) }
}

/// A session's canvases (`LiveSession`'s and `RemoteSession`'s `plugin_canvases`,
/// `show_canvas`, `hide_canvas`, `next_canvas` and `select_canvas`). The calls throw because a
/// remote session's can fail on the wire; a local one's do not.
public protocol CanvasClient: AnyObject, Sendable {
  func pluginCanvases() async throws -> [PluginCanvas]
  /// The pane shows `plugin`'s canvas at this size, newly or resized; the draw arrives through
  /// `nextCanvas`.
  func showCanvas(_ plugin: String, widthPt: UInt32, heightPt: UInt32) async throws
  func hideCanvas(_ plugin: String) async throws
  /// The next draw of any canvas; `nil` once the session is gone.
  func nextCanvas() async -> CanvasUpdate?
  /// A click on `frame` of `plugin`'s canvas, the clicked element's `value`: the composer's
  /// attachment, if the plugin gives one.
  func selectCanvas(_ plugin: String, frame: String, value: String) async throws -> CanvasAttach?
}
