// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// One session's plugin canvas pane as observable state (docs/design/plugin-canvas.md §3, §5, §6,
// T33.47.8.2): the canvases its plugins declared, the one the pane shows, that canvas's frames in
// the plugin's order, and whether it stopped. The core decides when to draw and what changed;
// this folds each update so an unchanged frame keeps its value, and so its web view, and hands a
// click's attachment to the composer as one chip per canvas.

import CoxClient
import Observation

@Observable
@MainActor
public final class CanvasStore {
  /// Every canvas the session's plugins declared, for the pane's picker and the palette.
  public private(set) var canvases: [PluginCanvas] = []
  /// The plugin whose canvas the pane shows; `nil` while it is hidden.
  public private(set) var shown: String?
  /// The shown canvas's frames, in the plugin's order.
  public private(set) var frames: [CanvasFrame] = []
  /// The plugin missed three draws in a row; its last frames stay, and it is not drawn again.
  public private(set) var stopped = false
  /// The canvas the palette picked while the pane was hidden, which it opens on.
  public private(set) var picked: String?
  /// Why the last call failed; a remote session's can.
  public private(set) var failure: String?
  @ObservationIgnored private let client: any CanvasClient
  @ObservationIgnored private let composer: ComposerStore
  /// The size the pane last showed the canvas at, so an unchanged one is not sent again.
  @ObservationIgnored private var size: [UInt32] = []

  public init(client: any CanvasClient, composer: ComposerStore) {
    (self.client, self.composer) = (client, composer)
  }

  /// `nil` for a session that has no canvases, such as an external agent's.
  public convenience init?(composer: ComposerStore) {
    guard let client = composer.session.session as? any CanvasClient else { return nil }
    self.init(client: client, composer: composer)
  }

  /// The heading of the canvas shown.
  public var canvas: PluginCanvas? { canvases.first { $0.plugin == shown } }

  public func load() async {
    do { canvases = try await client.pluginCanvases() } catch { report(error) }
  }

  /// Shows `plugin`'s canvas, else the one shown, else the one picked, else the first declared,
  /// at this size. Another plugin's canvas shown before is hidden first, so only one draws.
  public func show(_ plugin: String? = nil, widthPt: UInt32, heightPt: UInt32) async {
    guard let plugin = plugin ?? shown ?? picked ?? canvases.first?.plugin else { return }
    picked = nil
    if let shown, shown != plugin { await hide() }
    if shown != plugin { (frames, stopped) = ([], false) }
    shown = plugin
    size = [widthPt, heightPt]
    do {
      try await client.showCanvas(plugin, widthPt: widthPt, heightPt: heightPt)
    } catch {
      report(error)
    }
  }

  /// The palette's "Show <title>": only the pane knows its size, so a shown pane switches at the
  /// size it last sent, and a hidden one opens on `plugin` when it next shows.
  public func pick(_ plugin: String) async {
    guard shown != nil, size.count == 2 else {
      picked = plugin
      return
    }
    await show(plugin, widthPt: size[0], heightPt: size[1])
  }

  /// The pane's size changed; the core draws again when the change is large enough.
  public func resize(widthPt: UInt32, heightPt: UInt32) async {
    guard shown != nil, size != [widthPt, heightPt] else { return }
    await show(widthPt: widthPt, heightPt: heightPt)
  }

  public func hide() async {
    guard let plugin = shown else { return }
    (shown, frames, stopped, size) = (nil, [], false, [])
    do { try await client.hideCanvas(plugin) } catch { report(error) }
  }

  /// Folds every draw until the session is gone; run once for the session's life.
  public func pump() async {
    while let update = await client.nextCanvas() { apply(update) }
  }

  /// A click on `frame`, the clicked element's `value`: the plugin's attachment, if any, replaces
  /// the chip this canvas added before. Only a frame on the canvas now is asked about.
  public func select(frame: String, value: String) async {
    guard let plugin = shown, frames.contains(where: { $0.id == frame }) else { return }
    do {
      guard let attach = try await client.selectCanvas(plugin, frame: frame, value: value) else {
        return
      }
      composer.attachSelection(attach, from: plugin)
    } catch {
      report(error)
    }
  }

  func apply(_ update: CanvasUpdate) {
    // A canvas hidden since the draw started, or another plugin's: not what the pane shows.
    guard update.plugin == shown else { return }
    guard !update.stopped else {
      stopped = true
      return
    }
    let changed = Dictionary(update.frames.map { ($0.id, $0) }) { _, last in last }
    let kept = Dictionary(frames.map { ($0.id, $0) }) { first, _ in first }
    frames = update.order.compactMap { changed[$0] ?? kept[$0] }
  }

  private func report(_ error: any Error) {
    failure = String(describing: error)
  }
}
