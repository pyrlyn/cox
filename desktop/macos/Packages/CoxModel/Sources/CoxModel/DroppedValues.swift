// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Project values the guard list threw out (T37.30.4, DT§5.7), on the page Rust put their key on:
// the key, why the project may not set it, and Rust's `999 → 5`. Here, not in CoxUI, because
// CoxUI depends on no cox package; the app copies them into CoxUI's `SettingsScreen.DroppedValue`
// field for field.

import CoxClient

public struct DroppedRow: Identifiable, Equatable, Sendable {
  /// Dotted, as the project file names it.
  public let key: String
  /// Why the project may not set it, from Rust's guard list.
  public let reason: String
  /// `999 → 5`: what the project set, then what holds.
  public let change: String
  public var id: String { key }
}

extension SettingsStore {
  /// The values the project set under `group` that the guard list dropped, in Rust's order.
  public func dropped(in group: SettingsGroup) -> [DroppedRow] {
    (view?.dropped ?? []).filter { $0.group == group }.map {
      DroppedRow(key: $0.key, reason: $0.reason, change: $0.change)
    }
  }
}
