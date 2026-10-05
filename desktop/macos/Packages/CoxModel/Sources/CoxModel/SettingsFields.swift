// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Settings screen's fields (DT§5.7): a sidebar group split into one box per config table,
// each field with the title, detail line and control Rust gave it (T58.4.8–T58.4.9). Here, not
// in CoxUI, because CoxUI depends on no cox package; the app copies them into CoxUI's
// `SettingsScreenState` case for case.

import CoxClient
import OrderedCollections

/// What a field shows; CoxUI's `SettingsScreen.Control` has the same cases.
public typealias SettingControl = Setting.Control

/// A pop-up's option: the value sent, and what the menu calls it.
public typealias SettingOption = Setting.Option

public struct SettingsField: Identifiable, Equatable, Sendable {
  public let setting: Setting
  /// The key's last segment in words: `base_url` → `Base url`.
  public let title: String
  /// For a value the project sets, the file it is set in; otherwise the schema's help text.
  public let detail: String?
  public let control: SettingControl
  public var id: String { setting.key }
}

/// One box on a group's page: the settings of one config table.
public struct SettingsTable: Identifiable, Equatable, Sendable {
  /// The keys' shared prefix, `tiers.code`.
  public let name: String
  public let fields: [SettingsField]
  /// The provider section whose key the box takes, for a `providers.<name>` table.
  public let provider: String?
  public var id: String { name }
}

extension SettingsStore {
  /// `section`'s settings by the table Rust put each in, each table where its first key sorts;
  /// a setting with no table (a rule list) is the Permissions page's own box.
  public func tables(in section: SettingsSection) -> [SettingsTable] {
    let rows = section.settings.filter { $0.table != nil }
    let byTable = OrderedDictionary(grouping: rows) { $0.table ?? "" }
    return byTable.map { name, settings in
      SettingsTable(
        name: name,
        fields: settings.map {
          SettingsField(
            setting: $0, title: $0.title, detail: $0.detail, control: $0.control)
        },
        provider: settings.first?.provider)
    }
  }
}

extension RuleKind {
  /// The dotted key of this kind's list.
  public var key: String { "permissions.\(rawValue)" }
}
