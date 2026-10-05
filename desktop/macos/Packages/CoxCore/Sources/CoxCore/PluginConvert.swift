// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A plugin slot's generated cox-ffi values ⇄ `CoxClient` values (PL§8, T52.17), field for
// field like `Convert.swift`: the slot, its kind and the sanitized widget tree. Separate so the
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
    }
  }
}

extension CoxClient.PluginRun {
  init(_ value: CoxFFIBindings.SpanView) {
    self.init(value.text, token: .init(value.style), bold: value.bold, italic: value.italic)
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
    case .block(let title, let child):
      self = .block(
        title: title.map { CoxClient.PluginRun($0) }, child: child.map { CoxClient.PluginView($0) })
    }
  }
}
