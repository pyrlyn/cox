// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A plugin slot's generated cox-ffi values ⇄ `CoxClient` values (PL§8, T52.17), field for
// field like `Convert.swift`: the slot, its kind and the sanitized widget tree, and the desktop
// contributions (PL§15.4) — tabs, toolbar items, link clicks and notices — and the canvases
// (plugin-canvas §8, T33.47.8.3), whose frames' HTML passes through untouched. Separate so the
// timeline's conversions stay one file; the terminal cell widths and stack sizes are dropped,
// since the app lays the tree out natively.

import CoxClient
import CoxFFIBindings

extension CoxClient.PluginSlot {
  init(_ value: CoxFFIBindings.PluginSlot) {
    self.init(
      plugin: value.plugin, slot: .init(value.slot),
      view: value.view.map { CoxClient.PluginView($0) },
      visible: value.visible, stopped: value.stopped)
  }
}

extension CoxClient.PluginSlotKind {
  init(_ value: CoxFFIBindings.Slot) {
    switch value {
    case .statusLeft: self = .statusLeft
    case .statusRight: self = .statusRight
    case .panel: self = .panel
    case .overlay: self = .overlay
    case .desktopInspector: self = .desktopInspector
    }
  }
}

extension CoxClient.PluginRun {
  init(_ value: CoxFFIBindings.SpanView) {
    self.init(
      value.text, token: .init(value.style), bold: value.bold, italic: value.italic,
      link: value.link)
  }
}

extension CoxClient.PluginView {
  init(_ value: CoxFFIBindings.WidgetView) {
    let runs = { (spans: [CoxFFIBindings.SpanView]) in spans.map { CoxClient.PluginRun($0) } }
    switch value {
    case .text(let lines): self = .text(lines: lines.map(runs))
    case .list(let items, let selected): self = .list(items: items.map(runs), selected: selected)
    case .table(let header, let rows, _): self = .table(header: runs(header), rows: rows.map(runs))
    case .keyValue(let rows):
      self = .keyValue(rows: rows.map { .init(key: .init($0.key), value: runs($0.value)) })
    case .gauge(let ratio, let label): self = .gauge(ratio: ratio, label: .init(label))
    case .stack(let vertical, let children, _):
      self = .stack(vertical: vertical, children: children.map { CoxClient.PluginView($0) })
    case .image(let alt, let png): self = .image(alt: alt, png: [UInt8](png))
    case .block(let title, let child):
      self = .block(
        title: title.map { CoxClient.PluginRun($0) }, child: child.map { CoxClient.PluginView($0) })
    }
  }
}

extension CoxClient.PluginTab {
  init(_ value: CoxFFIBindings.PluginTab) {
    self.init(plugin: value.plugin, title: value.title, symbol: value.symbol)
  }
}

extension CoxClient.PluginAction {
  init(_ value: CoxFFIBindings.PluginAction) {
    self.init(id: value.id, plugin: value.plugin, title: value.title, symbol: value.symbol)
  }
}

extension CoxClient.LinkClick {
  init(_ value: CoxFFIBindings.LinkClick) {
    switch value {
    case .opened: self = .opened
    case .file(let path, let line): self = .file(path: path, line: line)
    case .refused: self = .refused
    }
  }
}

extension CoxClient.PluginAlert {
  init(_ value: CoxFFIBindings.PluginAlert) {
    self.init(
      session: value.session, plugin: value.plugin, warn: value.warn, title: value.title,
      body: value.body, actions: value.actions.map { .init(label: $0.label, id: $0.id) })
  }
}

extension CoxClient.PluginCanvas {
  init(_ value: CoxFFIBindings.PluginCanvas) {
    self.init(
      plugin: value.plugin, title: value.title, symbol: value.symbol, selectAttr: value.selectAttr)
  }
}

extension CoxClient.CanvasUpdate {
  init(_ value: CoxFFIBindings.CanvasUpdate) {
    self.init(
      plugin: value.plugin, order: value.order,
      frames: value.frames.map {
        CanvasFrame(id: $0.id, title: $0.title, widthPt: $0.widthPt, html: $0.html)
      },
      stopped: value.stopped)
  }
}

extension CoxClient.CanvasAttach {
  init(_ value: CoxFFIBindings.CanvasAttach) {
    self.init(name: value.name, text: value.text)
  }
}
