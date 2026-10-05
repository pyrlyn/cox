// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The command palette's rows between CoxClient and cox-ffi (T37.44.13), apart from
// `Convert.swift`'s timeline so each file stays one concern. Case for case; the ranking is
// `cox_app::palette::rank`, which both the live and the remote session call through here.

import CoxClient
import CoxFFIBindings

extension CoxFFIBindings.PaletteItem {
  init(_ value: CoxClient.PaletteItem) {
    let kind: CoxFFIBindings.PaletteKind =
      switch value.kind {
      case .action: .action
      case .session: .session
      case .command: .command
      case .file: .file
      }
    self.init(kind: kind, id: value.id, title: value.title, detail: value.detail)
  }
}

extension CoxClient.PaletteHit {
  init(_ value: CoxFFIBindings.PaletteHit) {
    let kind: CoxClient.PaletteKind =
      switch value.item.kind {
      case .action: .action
      case .session: .session
      case .command: .command
      case .file: .file
      }
    self.init(
      item: CoxClient.PaletteItem(
        kind: kind, id: value.item.id, title: value.item.title, detail: value.item.detail),
      matched: value.matched.map { Int($0) })
  }
}

extension CoxClient.PaletteHit {
  /// A session without its own completer (a remote one): the window's actions and sessions,
  /// ranked by the same Rust function with no commands or files added.
  static func ranked(_ query: String, _ items: [CoxClient.PaletteItem], limit: UInt32) -> [Self] {
    CoxFFIBindings.palette(query: query, items: items.map { .init($0) }, limit: limit)
      .map { Self($0) }
  }
}
