// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The session window's plugin canvas pane (T33.47.8.5, docs/design/plugin-canvas.md §3–§5):
// CoxUI's `PluginCanvasPane` around one WebKit `WebView` per frame, each over CoxPlatform's
// isolated `CanvasPage`. Wiring only: `CanvasStore` folds the plugin's draws, a page loads again
// only when its frame's HTML changed, and a click's value goes to the store with the frame id
// this view owns, never one the page names.

import CoxClient
import CoxModel
import CoxPlatform
import CoxUI
import SwiftUI
import WebKit

struct SessionCanvas: View {
  /// As wide as the browser pane, narrower when the window is.
  static let paneWidth = SessionBrowser.paneWidth
  /// A frame's height until its page is measured.
  static let unmeasuredHeight: CGFloat = 320

  let canvas: CanvasStore
  let refused: (String) -> Void
  @State private var pages = CanvasPages()
  @State private var zoom = 1.0
  @State private var size = CGSize.zero

  var body: some View {
    PluginCanvasPane(state: state, send: handle) { frame in
      if let page = pages.pages[frame.id] { WebView(page.page) }
    }
    .onGeometryChange(for: CGSize.self, of: \.size) { size = $0 }
    .task(id: size) { await place() }
    .task(id: canvas.frames) { await sync() }
    .task(id: zoom) {
      do { try await pages.zoom(zoom) } catch { refused(String(describing: error)) }
    }
    .onChange(of: canvas.failure) { if let failure = canvas.failure { refused(failure) } }
    .onDisappear {
      pages.clear()
      Task { await canvas.hide() }
    }
  }

  private var state: PluginCanvasPaneState {
    PluginCanvasPaneState(
      canvases: canvas.canvases.map { .init(id: $0.plugin, title: $0.title, symbol: $0.symbol) },
      shown: canvas.shown ?? "",
      frames: canvas.frames.map { frame in
        .init(
          id: frame.id, title: frame.title, width: CGFloat(frame.widthPt),
          height: pages.heights[frame.id] ?? Self.unmeasuredHeight)
      },
      stopped: canvas.stopped, zoom: zoom)
  }

  /// The pane's size in whole points; `nil` before it is laid out.
  private var points: (width: UInt32, height: UInt32)? {
    guard size.width >= 1, size.height >= 1, size.width.isFinite, size.height.isFinite else {
      return nil
    }
    return (UInt32(size.width.rounded()), UInt32(size.height.rounded()))
  }

  /// Shows the canvas once the pane has a size, and sends each new size after; the core decides
  /// whether a change is large enough to draw again.
  private func place() async {
    guard let points else { return }
    if canvas.shown == nil {
      if canvas.canvases.isEmpty { await canvas.load() }
      await canvas.show(widthPt: points.width, heightPt: points.height)
    } else {
      await canvas.resize(widthPt: points.width, heightPt: points.height)
    }
  }

  private func sync() async {
    guard let plugin = canvas.canvas else {
      pages.clear()
      return
    }
    do {
      try await pages.sync(canvas.frames, of: plugin, zoom: zoom) { frame, value in
        await canvas.select(frame: frame, value: value)
      }
    } catch {
      // A newer draw cancelled this one, and its load replaced the one that failed.
      if !Task.isCancelled { refused(String(describing: error)) }
    }
  }

  private func handle(_ intent: PluginCanvasPaneIntent) {
    switch intent {
    case .pick(let plugin):
      guard let points else { return }
      Task { await canvas.show(plugin, widthPt: points.width, heightPt: points.height) }
    case .zoom(let zoom): self.zoom = zoom
    }
  }
}

/// The pane's pages, one per frame id, kept while the frame is on the canvas, so an unchanged
/// frame keeps its page, its scroll and its selection's outline.
@Observable
@MainActor
final class CanvasPages {
  private(set) var pages: [String: CanvasPage] = [:]
  /// Each page's content height at 100%, once measured.
  private(set) var heights: [String: CGFloat] = [:]
  /// The HTML each page shows, so an unchanged frame is not loaded again.
  @ObservationIgnored private var loaded: [String: String] = [:]
  @ObservationIgnored private var listeners: [String: Task<Void, Never>] = [:]
  /// Another plugin's canvas reads another attribute, so its frames get new pages.
  @ObservationIgnored private var plugin: String?

  func sync(
    _ frames: [CanvasFrame], of canvas: PluginCanvas, zoom: Double,
    select: @escaping @MainActor (String, String) async -> Void
  ) async throws(CanvasFailure) {
    if plugin != canvas.plugin {
      clear()
      plugin = canvas.plugin
    }
    let ids = Set(frames.map(\.id))
    for id in pages.keys where !ids.contains(id) { drop(id) }
    for frame in frames where loaded[frame.id] != frame.html {
      let page = try await page(frame.id, attribute: canvas.selectAttr, select: select)
      let height = try await page.show(html: frame.html, zoom: zoom)
      (heights[frame.id], loaded[frame.id]) = (CGFloat(height), frame.html)
      if Task.isCancelled { return }
    }
  }

  func zoom(_ zoom: Double) async throws(CanvasFailure) {
    for page in pages.values { try await page.zoom(zoom) }
  }

  func clear() {
    for id in pages.keys { drop(id) }
    plugin = nil
  }

  private func page(
    _ id: String, attribute: String, select: @escaping @MainActor (String, String) async -> Void
  ) async throws(CanvasFailure) -> CanvasPage {
    if let page = pages[id] { return page }
    let made = try await CanvasPage.make(selectAttribute: attribute)
    // A sync a newer draw cancelled may have made one meanwhile; the first stays.
    if let page = pages[id] { return page }
    pages[id] = made
    listeners[id] = Task { for await value in made.selections { await select(id, value) } }
    return made
  }

  private func drop(_ id: String) {
    listeners.removeValue(forKey: id)?.cancel()
    (pages[id], heights[id], loaded[id]) = (nil, nil, nil)
  }
}
