// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PluginCanvasPane` (docs/design/plugin-canvas.md §3, T33.47.8.4): a plugin's canvas beside the
// transcript. A bar holds the plugin's symbol and title, a picker when more than one plugin
// declares a canvas, and the zoom; under it the frames sit side by side, each at its width under
// its title. Separate because each frame's page is WebKit's `WebView` over CoxPlatform's isolated
// `CanvasPage`, which CoxUI does not link (DT§4.6): the app passes it in through `page`, with
// the zoom to apply inside it, since scaling a web view from outside would misplace its clicks.

import SwiftUI

/// The bar is `Size.toolbarHeight` tall. Below it, the frames scroll both ways; each page sits in
/// a `radius.l` well with a hairline rim, its title above it in `font.caption`.
public struct PluginCanvasPane<Page: View>: View {
  public typealias State = PluginCanvasPaneState
  public typealias Intent = PluginCanvasPaneIntent

  let state: State
  let send: (Intent) -> Void
  let page: (State.Frame) -> Page

  public init(
    state: State, send: @escaping (Intent) -> Void,
    @ViewBuilder page: @escaping (State.Frame) -> Page
  ) {
    (self.state, self.send, self.page) = (state, send, page)
  }

  public var body: some View {
    VStack(spacing: 0) {
      bar
      if state.stopped {
        Text("\(state.title) stopped drawing")
          .textStyle(.caption)
          .foregroundStyle(Color(.textSecondary))
          .padding(.bottom, Space.s)
      }
      ScrollView([.horizontal, .vertical]) {
        HStack(alignment: .top, spacing: Space.xl) {
          ForEach(state.frames) { frame in framed(frame) }
        }
        .padding(Space.l)
      }
    }
    .focusable()
    .focusEffectDisabled()
    .onKeyPress(characters: ["+", "=", "-"]) { press in
      guard press.modifiers.contains(.command) else { return .ignored }
      send(.zoom(State.zoomed(state.zoom, in: press.characters != "-")))
      return .handled
    }
    .gesture(
      MagnifyGesture().onEnded { value in
        send(.zoom(State.clamped(state.zoom * value.magnification)))
      }
    )
    .accessibilityElement(children: .contain)
    .accessibilityLabel(state.title)
  }

  private var bar: some View {
    HStack(spacing: Space.m) {
      Image(systemName: PluginSymbol.name(state.symbol))
        .symbolStyle(.caption)
        .foregroundStyle(Color(.textSecondary))
        .accessibilityHidden(true)
      if state.canvases.count > 1 {
        Picker("Canvas", selection: Binding(get: { state.shown }, set: { send(.pick($0)) })) {
          ForEach(state.canvases) { Text($0.title).tag($0.id) }
        }
        .pickerStyle(.menu)
        .labelsHidden()
        .fixedSize()
      } else {
        Text(state.title).textStyle(.titleCard).foregroundStyle(Color(.textPrimary))
      }
      Spacer(minLength: 0)
      zoomButton("minus.magnifyingglass", "Zoom out", zoomIn: false)
      Text("\(Int((state.zoom * 100).rounded()))%")
        .textStyle(.caption)
        .monospacedDigit()
        .foregroundStyle(Color(.textSecondary))
      zoomButton("plus.magnifyingglass", "Zoom in", zoomIn: true)
    }
    .padding(.horizontal, Space.l)
    .frame(height: Size.toolbarHeight)
  }

  private func zoomButton(_ symbol: String, _ label: String, zoomIn: Bool) -> some View {
    Button {
      send(.zoom(State.zoomed(state.zoom, in: zoomIn)))
    } label: {
      Image(systemName: symbol).symbolStyle(.caption)
    }
    .buttonStyle(CapsuleStyle(isIcon: true))
    .disabled(State.zoomed(state.zoom, in: zoomIn) == state.zoom)
    .accessibilityLabel(label)
  }

  private func framed(_ frame: State.Frame) -> some View {
    let shape = RoundedRectangle(cornerRadius: Radius.l, style: .continuous)
    return VStack(alignment: .leading, spacing: Space.s) {
      Text(frame.title)
        .textStyle(.caption)
        .foregroundStyle(Color(.textSecondary))
        .lineLimit(1)
      // The app's page fills the box and zooms inside; a page laid out at 100% keeps its corner.
      page(frame)
        .frame(
          width: frame.width * state.zoom, height: frame.height * state.zoom, alignment: .topLeading
        )
        .clipShape(shape)
        .hairline(in: shape)
    }
    .frame(width: frame.width * state.zoom, alignment: .leading)
  }
}

/// What `PluginCanvasPane` shows. It sits outside the generic view so the app and the previews
/// share one type whatever the pages are.
public struct PluginCanvasPaneState: Equatable, Sendable {
  /// A declared canvas, for the picker; `id` is its plugin's.
  public struct Canvas: Equatable, Sendable, Identifiable {
    public var id: String
    public var title: String
    public var symbol: String

    public init(id: String, title: String, symbol: String) {
      (self.id, self.title, self.symbol) = (id, title, symbol)
    }
  }

  /// A frame's place: the plugin's width and the page's content height, in points at 100%.
  public struct Frame: Equatable, Sendable, Identifiable {
    public var id: String
    public var title: String
    public var width: CGFloat
    public var height: CGFloat

    public init(id: String, title: String, width: CGFloat, height: CGFloat) {
      (self.id, self.title, self.width, self.height) = (id, title, width, height)
    }
  }

  public var canvases: [Canvas]
  /// The shown canvas's plugin.
  public var shown: String
  public var frames: [Frame]
  public var stopped: Bool
  public var zoom: Double

  /// From a quarter to twice the size, in steps of a quarter.
  static let zoomRange = 0.25...2.0
  static let zoomStep = 0.25

  public init(
    canvases: [Canvas], shown: String, frames: [Frame] = [], stopped: Bool = false,
    zoom: Double = 1
  ) {
    (self.canvases, self.shown, self.frames) = (canvases, shown, frames)
    (self.stopped, self.zoom) = (stopped, Self.clamped(zoom))
  }

  var canvas: Canvas? { canvases.first { $0.id == shown } }
  var title: String { canvas?.title ?? "Canvas" }
  var symbol: String? { canvas?.symbol }

  /// One step in or out, on the steps' grid, within the range.
  static func zoomed(_ zoom: Double, in zoomIn: Bool) -> Double {
    let steps = (zoom / zoomStep).rounded() + (zoomIn ? 1 : -1)
    return clamped(steps * zoomStep)
  }

  static func clamped(_ zoom: Double) -> Double {
    min(max(zoom, zoomRange.lowerBound), zoomRange.upperBound)
  }
}

/// What `PluginCanvasPane` reports.
public enum PluginCanvasPaneIntent: Equatable, Sendable {
  /// The picker chose this plugin's canvas.
  case pick(String)
  /// The zoom the pane asks for, already clamped.
  case zoom(Double)
}

extension ShellShortcut {
  /// The canvas pane's toggle (plugin-canvas §3, DT§5.5). The View menu's command answers it.
  public static let canvas = Self(key: .init("d", modifiers: [.command, .shift]), glyphs: "⌘⇧D")
}

#Preview("two frames") {
  PreviewMatrix { PluginCanvasSample(state: PreviewState.canvasTwoFrames) }
}
#Preview("picker, stopped") {
  PreviewMatrix { PluginCanvasSample(state: PreviewState.canvasPicker) }
}
